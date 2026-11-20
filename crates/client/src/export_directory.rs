use crate::{
    Client, Error, ProtectedDir,
    protocol::{export as format, hex, unhex},
};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{self, BufWriter, Write},
};

impl Client {
    /// Write a completed owner export to a new local directory.
    pub async fn export_directory(&self, path: &str) -> Result<Value, Error> {
        let mut output = None;
        self.export(|record| {
            if record["type"] == "header" {
                output = Some(Output::new(path)?);
                Ok(())
            } else {
                output.as_mut().ok_or_else(invalid)?.record(record)
            }
        })
        .await?;
        let manifest = output.ok_or_else(invalid)?.finish()?;
        Ok(json!({"directory":path, "export":manifest}))
    }
}

struct Output {
    root: ProtectedDir,
    bodies: ProtectedDir,
    metadata: BufWriter<File>,
    body_count: u64,
    record_count: u64,
}

impl Output {
    fn new(path: &str) -> io::Result<Self> {
        let root = ProtectedDir::create(path)?;
        let bodies = ProtectedDir::create(root.path().join("bodies"))?;
        let metadata = BufWriter::new(root.create_file("metadata.jsonl")?);
        Ok(Self {
            root,
            bodies,
            metadata,
            body_count: 0,
            record_count: 0,
        })
    }

    fn record(&mut self, mut record: Value) -> io::Result<()> {
        if record["type"] == "body" {
            let id = record["content_id"]
                .as_str()
                .and_then(|id| unhex::<32>(id).ok())
                .ok_or_else(invalid)?;
            let name = format!("{}.txt", hex(&id));
            let object = record.as_object_mut().ok_or_else(invalid)?;
            let text = object.remove("text").ok_or_else(invalid)?;
            self.bodies
                .write_new(&name, text.as_str().ok_or_else(invalid)?.as_bytes())?;
            object.insert("file".into(), json!(format!("bodies/{name}")));
            self.body_count += 1;
        }
        serde_json::to_writer(&mut self.metadata, &record)?;
        self.metadata.write_all(b"\n")?;
        self.record_count += 1;
        Ok(())
    }

    fn finish(mut self) -> io::Result<Value> {
        self.metadata.flush()?;
        self.metadata.get_ref().sync_all()?;
        let manifest = json!({
            "format":format::FORMAT, "version":format::VERSION, "scope":format::SCOPE,
            "metadata":"metadata.jsonl", "records":self.record_count, "body_files":self.body_count,
        });
        // Publishing the manifest is the completion point for a new directory.
        self.root
            .replace("manifest.json", &serde_json::to_vec_pretty(&manifest)?)?;
        File::open(self.root.path().parent().ok_or_else(invalid)?)?.sync_all()?;
        Ok(manifest)
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid export record")
}
