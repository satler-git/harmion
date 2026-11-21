use super::*;
use harmion_node::sync_status::{Failure, Observation};

pub(super) fn initialize(sql: &Connection) -> Result<(), Error> {
    sql.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_observations (
        node BLOB PRIMARY KEY REFERENCES peers(node) CHECK(length(node)=32),
        local_epoch BLOB NOT NULL CHECK(length(local_epoch)=32),
        acknowledged_through INTEGER NOT NULL CHECK(acknowledged_through>=0),
        remote_epoch BLOB NOT NULL CHECK(length(remote_epoch)=32),
        remote_high INTEGER NOT NULL CHECK(remote_high>=0),
        received_through INTEGER NOT NULL CHECK(received_through>=remote_high),
        succeeded_at INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS sync_failures (
        node BLOB PRIMARY KEY REFERENCES peers(node) CHECK(length(node)=32),
        attempted_at INTEGER NOT NULL, code TEXT NOT NULL);",
    )
    .map_err(db)
}

impl SqlTransaction<'_> {
    pub(super) fn read_sync_observation(&self, node: NodeId) -> Result<Option<Observation>, Error> {
        let row = self.sql.query_row(
            "SELECT local_epoch,acknowledged_through,remote_epoch,remote_high,received_through,succeeded_at FROM sync_observations WHERE node=?1",
            [node.0], |row| Ok((row.get::<_,Vec<u8>>(0)?,row.get::<_,i64>(1)?,row.get::<_,Vec<u8>>(2)?,row.get::<_,i64>(3)?,row.get::<_,i64>(4)?,row.get::<_,i64>(5)?))
        ).optional().map_err(db)?;
        row.map(
            |(local, acknowledged_through, remote, remote_high, received_through, succeeded_at)| {
                Ok(Observation {
                    local_epoch: LogEpoch(fixed(local)?),
                    acknowledged_through: acknowledged_through
                        .try_into()
                        .map_err(|_| Error::CorruptLog)?,
                    remote_epoch: LogEpoch(fixed(remote)?),
                    remote_high: remote_high.try_into().map_err(|_| Error::CorruptLog)?,
                    received_through: received_through.try_into().map_err(|_| Error::CorruptLog)?,
                    succeeded_at,
                })
            },
        )
        .transpose()
    }

    pub(super) fn write_sync_observation(
        &self,
        node: NodeId,
        observation: &Observation,
    ) -> Result<(), Error> {
        self.sql.execute("INSERT INTO sync_observations VALUES(?1,?2,?3,?4,?5,?6,?7)
            ON CONFLICT(node) DO UPDATE SET local_epoch=excluded.local_epoch,acknowledged_through=excluded.acknowledged_through,
            remote_epoch=excluded.remote_epoch,remote_high=excluded.remote_high,received_through=excluded.received_through,succeeded_at=excluded.succeeded_at",
            params![node.0,observation.local_epoch.0,seq(observation.acknowledged_through)?,observation.remote_epoch.0,seq(observation.remote_high)?,seq(observation.received_through)?,observation.succeeded_at]).map_err(db)?;
        Ok(())
    }

    pub(super) fn read_sync_failure(&self, node: NodeId) -> Result<Option<Failure>, Error> {
        self.sql
            .query_row(
                "SELECT attempted_at,code FROM sync_failures WHERE node=?1",
                [node.0],
                |row| {
                    Ok(Failure {
                        attempted_at: row.get(0)?,
                        code: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(db)
    }

    pub(super) fn write_sync_failure(
        &self,
        node: NodeId,
        failure: Option<&Failure>,
    ) -> Result<(), Error> {
        if let Some(failure) = failure {
            self.sql.execute("INSERT INTO sync_failures VALUES(?1,?2,?3) ON CONFLICT(node) DO UPDATE SET attempted_at=excluded.attempted_at,code=excluded.code", params![node.0,failure.attempted_at,failure.code]).map_err(db)?;
        } else {
            self.sql
                .execute("DELETE FROM sync_failures WHERE node=?1", [node.0])
                .map_err(db)?;
        }
        Ok(())
    }
}
