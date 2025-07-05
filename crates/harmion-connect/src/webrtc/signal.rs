use std::net;

struct SignalInfo {
    addr: net::SocketAddr,
}

struct Signal {
    port: u16,
}

impl Signal {
    fn new(port: u16) -> Self {
        Self { port }
    }

    fn info(&self) -> SignalInfo {
        // Self {
        //     addr: net::SocketAddr(IpAddr::)
        // }
        // TODO: get_if_addrs
        todo!()
    }
}
