mod signal;
mod simple;

mod error {
    use thiserror::Error;

    #[derive(Error, Debug)]
    pub(crate) enum WebRTCError {}
}

pub(crate) struct Config {}

pub(crate) struct WebRTCConn {
    config: Config,
}

#[cfg(test)]
mod tests {
    // use crate::Connection;

    // #[test]
    // fn webrtc_conn() -> Result<(), Box<dyn std::error::Error>> {
    //     __test(super::WebRTCConn);
    //     Ok(())
    // }
}
