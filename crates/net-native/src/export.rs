use crate::{Error, SharedStore, SystemHost};
use bytes::Bytes;
use harmion_node::{access::Credential, application::Host, storage::Store};
use http_body_util::BodyExt;
use hyper::{Response, body::Frame};
use std::{
    io::{self, BufWriter, Write},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{Semaphore, mpsc, watch};

pub(crate) type Body = http_body_util::combinators::UnsyncBoxBody<Bytes, io::Error>;

pub(crate) async fn response<S: Store + Send + 'static>(
    store: &SharedStore<S>,
    credential: Credential,
    slot: Arc<Semaphore>,
    activity: watch::Sender<()>,
) -> Result<Response<Body>, Error> {
    let start_credential = credential.clone();
    let (snapshot, permit) = store
        .run(move |store| {
            let snapshot = harmion_node::export::Snapshot::start(
                store,
                &start_credential,
                SystemHost.now_micros(),
            )?;
            let permit = slot
                .try_acquire_owned()
                .map_err(|_| harmion_node::Error::ExportBusy)?;
            Ok((snapshot, permit))
        })
        .await?;
    let (sender, receiver) = mpsc::channel(2);
    let store = store.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let output = BufWriter::with_capacity(64 * 1024, ChannelWriter(&sender));
        if let Err(error) = stream(&store, &snapshot, &credential, output) {
            let _ = sender.blocking_send(Err(error));
        }
    });
    activity.send_replace(());
    Ok(Response::builder()
        .header("content-type", harmion_protocol::export::CONTENT_TYPE)
        .header("cache-control", "no-store")
        .header("x-content-type-options", "nosniff")
        .body(ExportBody { receiver, activity }.boxed_unsync())
        .expect("static headers"))
}

fn stream<S: Store + Send + 'static>(
    store: &SharedStore<S>,
    snapshot: &harmion_node::export::Snapshot,
    credential: &Credential,
    output: impl Write,
) -> io::Result<()> {
    let mut writer = harmion_protocol::export::ExportWriter::start(output)?;
    let mut after = None;
    loop {
        let ids = store
            .run_blocking(|db| snapshot.ids(db, credential, SystemHost.now_micros(), after))
            .map_err(io::Error::other)?;
        if ids.is_empty() {
            return writer.finish();
        }
        for id in ids {
            let graph = store
                .run_blocking(|db| snapshot.content(db, credential, SystemHost.now_micros(), id))
                .map_err(io::Error::other)?;
            writer.content(&graph, id)?;
            after = Some(id);
        }
    }
}

type Chunk = Result<Bytes, io::Error>;
struct ChannelWriter<'a>(&'a mpsc::Sender<Chunk>);

impl Write for ChannelWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let count = bytes.len().min(64 * 1024);
        self.0
            .blocking_send(Ok(Bytes::copy_from_slice(&bytes[..count])))
            .map_err(|_| io::ErrorKind::BrokenPipe)?;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct ExportBody {
    receiver: mpsc::Receiver<Chunk>,
    activity: watch::Sender<()>,
}

impl hyper::body::Body for ExportBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        match self.receiver.poll_recv(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                self.activity.send_replace(());
                Poll::Ready(Some(Ok(Frame::data(bytes))))
            }
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(error))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
