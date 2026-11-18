//! SQLite persistence for expiring, node-local OAuth grants.

use crate::{SqlTransaction, db, fixed};
use harmion_node::{
    Error,
    oauth::{Access, Authorization, Client, Code, Refresh, Storage},
};
use harmion_protocol::{AppId, GrantSpec};
use rusqlite::{Connection, OptionalExtension, params};

pub(super) fn initialize(sql: &Connection) -> Result<(), Error> {
    sql.execute_batch(
        "CREATE TABLE oauth_clients (
            app BLOB PRIMARY KEY NOT NULL REFERENCES apps(app_id),
            name TEXT NOT NULL, redirect TEXT NOT NULL);
        CREATE TABLE oauth_pending (
            request BLOB PRIMARY KEY NOT NULL CHECK(length(request)=32),
            app BLOB NOT NULL REFERENCES oauth_clients(app),
            redirect TEXT NOT NULL, challenge BLOB NOT NULL CHECK(length(challenge)=32),
            state TEXT NOT NULL, grants TEXT NOT NULL, expires_at INTEGER NOT NULL);
        CREATE INDEX oauth_pending_expiry ON oauth_pending(expires_at);
        CREATE TABLE oauth_codes (
            digest BLOB PRIMARY KEY NOT NULL CHECK(length(digest)=32),
            app BLOB NOT NULL REFERENCES oauth_clients(app),
            redirect TEXT NOT NULL, challenge BLOB NOT NULL CHECK(length(challenge)=32),
            generation INTEGER NOT NULL CHECK(generation>=0),
            expires_at INTEGER NOT NULL, family BLOB CHECK(length(family)=32));
        CREATE INDEX oauth_code_expiry ON oauth_codes(expires_at);
        CREATE TABLE oauth_families (
            id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=32),
            app BLOB NOT NULL REFERENCES oauth_clients(app), expires_at INTEGER NOT NULL);
        CREATE INDEX oauth_family_app ON oauth_families(app);
        CREATE INDEX oauth_family_expiry ON oauth_families(expires_at);
        CREATE TABLE oauth_access (
            verifier BLOB PRIMARY KEY NOT NULL CHECK(length(verifier)=32),
            family BLOB NOT NULL REFERENCES oauth_families(id) ON DELETE CASCADE,
            expires_at INTEGER NOT NULL);
        CREATE INDEX oauth_access_family ON oauth_access(family);
        CREATE INDEX oauth_access_expiry ON oauth_access(expires_at);
        CREATE TABLE oauth_refresh (
            digest BLOB PRIMARY KEY NOT NULL CHECK(length(digest)=32),
            family BLOB NOT NULL REFERENCES oauth_families(id) ON DELETE CASCADE,
            used INTEGER NOT NULL CHECK(used IN (0,1)));
        CREATE INDEX oauth_refresh_family ON oauth_refresh(family);",
    )
    .map_err(db)
}

impl Storage for SqlTransaction<'_> {
    fn oauth_client(&mut self, id: AppId) -> Result<Option<Client>, Error> {
        self.sql
            .query_row(
                "SELECT name,redirect FROM oauth_clients WHERE app=?1",
                [id.0],
                |row| {
                    Ok(Client {
                        id,
                        name: row.get(0)?,
                        redirect: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(db)
    }

    fn put_oauth_client(&mut self, client: &Client) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO oauth_clients VALUES (?1,?2,?3)",
                params![client.id.0, client.name, client.redirect],
            )
            .map_err(db)?;
        Ok(())
    }

    fn oauth_pending(&mut self, request: [u8; 32]) -> Result<Option<Authorization>, Error> {
        let row = self.sql.query_row(
            "SELECT app,redirect,challenge,state,grants,expires_at FROM oauth_pending WHERE request=?1",
            [request], |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get(1)?, row.get::<_, Vec<u8>>(2)?,
                row.get(3)?, row.get::<_, String>(4)?, row.get(5)?)),
        ).optional().map_err(db)?;
        row.map(|(app, redirect, challenge, state, grants, expires_at)| {
            let grants: GrantSpec = serde_json::from_str(&grants).map_err(|_| Error::CorruptLog)?;
            Ok(Authorization {
                request,
                client: AppId(fixed(app)?),
                redirect,
                challenge: fixed(challenge)?,
                state,
                grants: grants.into(),
                expires_at,
            })
        })
        .transpose()
    }

    fn oauth_pending_count(&mut self, now: i64) -> Result<usize, Error> {
        let count: i64 = self
            .sql
            .query_row(
                "SELECT COUNT(*) FROM oauth_pending WHERE expires_at>?1",
                [now],
                |row| row.get(0),
            )
            .map_err(db)?;
        count.try_into().map_err(|_| Error::CorruptLog)
    }

    fn put_oauth_pending(&mut self, authorization: &Authorization) -> Result<(), Error> {
        let grants = serde_json::to_string(&GrantSpec::from(authorization.grants.clone()))
            .map_err(|_| Error::InvalidRequest)?;
        self.sql
            .execute(
                "INSERT INTO oauth_pending VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    authorization.request,
                    authorization.client.0,
                    authorization.redirect,
                    authorization.challenge,
                    authorization.state,
                    grants,
                    authorization.expires_at
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    fn delete_oauth_pending(&mut self, request: [u8; 32]) -> Result<(), Error> {
        self.sql
            .execute("DELETE FROM oauth_pending WHERE request=?1", [request])
            .map_err(db)?;
        Ok(())
    }

    fn oauth_code(&mut self, digest: [u8; 32]) -> Result<Option<Code>, Error> {
        let row = self.sql.query_row(
            "SELECT app,redirect,challenge,generation,expires_at,family FROM oauth_codes WHERE digest=?1",
            [digest], |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get(1)?, row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)?, row.get(4)?, row.get::<_, Option<Vec<u8>>>(5)?)),
        ).optional().map_err(db)?;
        row.map(
            |(app, redirect, challenge, generation, expires_at, family)| {
                Ok(Code {
                    client: AppId(fixed(app)?),
                    redirect,
                    challenge: fixed(challenge)?,
                    generation: generation.try_into().map_err(|_| Error::CorruptLog)?,
                    expires_at,
                    family: family.map(fixed).transpose()?,
                })
            },
        )
        .transpose()
    }

    fn put_oauth_code(&mut self, digest: [u8; 32], code: &Code) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO oauth_codes VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    digest,
                    code.client.0,
                    code.redirect,
                    code.challenge,
                    i64::try_from(code.generation).map_err(|_| Error::CorruptLog)?,
                    code.expires_at,
                    code.family,
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    fn consume_oauth_code(&mut self, digest: [u8; 32], family: [u8; 32]) -> Result<(), Error> {
        let changed = self
            .sql
            .execute(
                "UPDATE oauth_codes SET family=?2 WHERE digest=?1 AND family IS NULL",
                params![digest, family],
            )
            .map_err(db)?;
        if changed != 1 {
            return Err(Error::OAuthInvalidGrant);
        }
        Ok(())
    }

    fn oauth_access(&mut self, verifier: [u8; 32]) -> Result<Option<Access>, Error> {
        let row = self.sql.query_row(
            "SELECT f.app,t.expires_at FROM oauth_access t JOIN oauth_families f ON f.id=t.family WHERE t.verifier=?1",
            [verifier], |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get(1)?)),
        ).optional().map_err(db)?;
        row.map(|(app, expires_at)| {
            Ok(Access {
                app: AppId(fixed(app)?),
                expires_at,
            })
        })
        .transpose()
    }

    fn put_oauth_access(
        &mut self,
        verifier: [u8; 32],
        family: [u8; 32],
        expires_at: i64,
    ) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO oauth_access VALUES (?1,?2,?3)",
                params![verifier, family, expires_at],
            )
            .map_err(db)?;
        Ok(())
    }

    fn oauth_refresh(&mut self, digest: [u8; 32]) -> Result<Option<Refresh>, Error> {
        let row = self.sql.query_row(
            "SELECT f.id,f.app,f.expires_at,t.used FROM oauth_refresh t JOIN oauth_families f ON f.id=t.family WHERE t.digest=?1",
            [digest], |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?, row.get(2)?, row.get(3)?)),
        ).optional().map_err(db)?;
        row.map(|(family, app, expires_at, used)| {
            Ok(Refresh {
                family: fixed(family)?,
                app: AppId(fixed(app)?),
                expires_at,
                used,
            })
        })
        .transpose()
    }

    fn put_oauth_refresh(&mut self, digest: [u8; 32], family: [u8; 32]) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO oauth_refresh VALUES (?1,?2,0)",
                params![digest, family],
            )
            .map_err(db)?;
        Ok(())
    }

    fn consume_oauth_refresh(&mut self, digest: [u8; 32]) -> Result<(), Error> {
        let changed = self
            .sql
            .execute(
                "UPDATE oauth_refresh SET used=1 WHERE digest=?1 AND used=0",
                [digest],
            )
            .map_err(db)?;
        if changed != 1 {
            return Err(Error::OAuthInvalidGrant);
        }
        Ok(())
    }

    fn put_oauth_family(
        &mut self,
        family: [u8; 32],
        app: AppId,
        expires_at: i64,
    ) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO oauth_families VALUES (?1,?2,?3)",
                params![family, app.0, expires_at],
            )
            .map_err(db)?;
        Ok(())
    }

    fn revoke_oauth_family(&mut self, family: [u8; 32]) -> Result<(), Error> {
        self.sql
            .execute("DELETE FROM oauth_families WHERE id=?1", [family])
            .map_err(db)?;
        Ok(())
    }

    fn revoke_oauth_app(&mut self, app: AppId) -> Result<(), Error> {
        self.sql
            .execute("DELETE FROM oauth_families WHERE app=?1", [app.0])
            .map_err(db)?;
        self.sql
            .execute("DELETE FROM oauth_codes WHERE app=?1", [app.0])
            .map_err(db)?;
        self.sql
            .execute("DELETE FROM oauth_pending WHERE app=?1", [app.0])
            .map_err(db)?;
        Ok(())
    }

    fn prune_oauth(&mut self, now: i64) -> Result<(), Error> {
        for table in [
            "oauth_pending",
            "oauth_codes",
            "oauth_access",
            "oauth_families",
        ] {
            self.sql
                .execute(&format!("DELETE FROM {table} WHERE expires_at<=?1"), [now])
                .map_err(db)?;
        }
        Ok(())
    }
}
