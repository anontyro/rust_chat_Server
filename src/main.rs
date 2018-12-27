extern crate mio;
use mio::*;
use mio::tcp::*;
use std::net::SocketAddr;

struct WebSocketServer;

impl Handler for WebSocketServer {
    type Timeout = usize;
    type Message =();
}

fn main() {

    // setup the general event loop with the correct handler
    let mut event_loop = EventLoop::new().unwrap();
    let mut handler = WebSocketServer;

     // register the event loop to a socket with a TCP Listener
    let address = "0.0.0.0:10000".parse::<SocketAddr>().unwrap();
    let server_socket = TcpListener::bind(&address).unwrap();

    event_loop.register(&server_socket, 
        Token(0), // sockets unique identifier
        EventSet::readable(), // what we expect to do with this registered socket (read, write, listen, everything)
        PollOpt::edge() // edge: notifications only on new data arriving, level: notifications when any data is there
    ).unwrap();

    event_loop.run(&mut handler).unwrap(); // event loop with a mutable handler inside

}
