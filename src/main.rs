extern crate mio;
extern crate http_muncher;
extern crate sha1;
extern crate rustc_serialize;

use std::net::SocketAddr;
use std::collections::HashMap;
use std::cell::RefCell;
use std::rc::Rc;
use std::fmt;
use mio::*;
use mio::tcp::*;
use http_muncher::{Parser, ParserHandler};
use rustc_serialize::base64::{ToBase64, STANDARD};

#[derive(PartialEq)]
enum ClientState {
     AwaitingHandshake,
    HandshakeResponse,
    Connected
}

// generator method used to create a new SHA1 key using the key ref and a constant
// returns a base64 new key
fn gen_key(key: &String) -> String {
    let mut m = sha1::Sha1::new();
    let mut buf = [0u8; 20];

    m.update(key.as_bytes());
    m.update("258EAFA5-E914-47DA-95CA-C5AB0DC85B11".as_bytes());

    m.output(&mut buf);

    return buf.to_base64(STANDARD);

}

// HttpParser is stateful so a new one is required for each user
struct HttpParser {
    current_key: Option<String>,
    headers: Rc<RefCell<HashMap<String, String>>>
}

// Parser Handler is called whenever it gets new data
impl ParserHandler for HttpParser {

    fn on_header_field(&mut self, s: &[u8]) -> bool {
        self.current_key = Some(std::str::from_utf8(s).unwrap().to_string());
        true
    }

    fn on_header_value(&mut self, s: &[u8]) -> bool {
        // borrow will defer checks until runtime
        // this means we must becareful to only borrow once
        self.headers.borrow_mut()
            .insert(
                self.current_key.clone().unwrap(),
                std::str::from_utf8(s).unwrap().to_string()
            );
        true
    }

    fn on_headers_complete(&mut self) -> bool {
        false
    }

}

// will handle all user data and store in one place
struct WebsocketClient {
    socket: TcpStream,
    headers: Rc<RefCell<HashMap<String, String>>>,
    http_parser: Parser<HttpParser>,
    interest: EventSet,
    state: ClientState
}

impl WebsocketClient {
    // New method is similar to a constructor as it is called to create a new instance of WebsocketClient
    fn new(socket: TcpStream) -> WebsocketClient {
        let headers = Rc::new(RefCell::new(HashMap::new()));

        WebsocketClient {
            socket: socket,
            headers: headers.clone(),
            interest: EventSet::readable(),
            state: ClientState::AwaitingHandshake,
            http_parser: Parser::request(HttpParser{
                current_key: None,
                headers: headers.clone()
            })
        }
    }

    fn read(&mut self) {
        // loop to wait for data to listen to
        loop {
            let mut buf = [0; 2048]; // create a buffer and allocate space to it
            // try to read from the socket if values arrive
            match self.socket.try_read(&mut buf) {
                // as we us try error may be thrown
                Err(e) => {
                    println!("Error whilst reading socket {}",e )
                },
                // none when no more bytes to read
                Ok(None) => break,
                Ok(Some(len)) => {
                    // providing a slice of the data to the parser
                    self.http_parser.parse(&buf[0..len]);

                    // check if the connection has the upgrade header
                    if self.http_parser.is_upgrade() {
                        self.state = ClientState::HandshakeResponse; // update the client state

                        // change the client state from read to write
                        self.interest.remove(EventSet::readable());
                        self.interest.insert(EventSet::writable());

                        break;
                    }
                }
            }
        }
    }

    fn write(&mut self) {
        let headers = self.headers.borrow();
        // return the websocket header and generate a new key
        let response_key = gen_key(&headers.get("Sec-WebSocket-Key").unwrap());

        let response =  fmt::format(format_args!("HTTP/1.1 101 Switching Protocols\r\n\
                                                 Connection: Upgrade\r\n\
                                                 Sec-WebSocket-Accept: {}\r\n\
                                                 Upgrade: websocket\r\n\r\n", response_key));

        // write the response headers to the socket
        self.socket.try_write(response.as_bytes()).unwrap();

        self.state = ClientState::Connected; // set state to connected

        // change the interest back to readable now the headers are added
        self.interest.remove(EventSet::writable());
        self.interest.insert(EventSet::readable());
    }
}

struct WebSocketServer {
    socket: TcpListener, //
    clients: HashMap<Token, WebsocketClient>, //stores incoming data from the clients
    token_counter: usize // token generation counter that is used to store next token value (using sequential)
}

const SERVER_TOKEN: Token = Token(0);

impl Handler for WebSocketServer {
    type Timeout = usize;
    type Message =();
    
    // Overriding function 
    fn ready(&mut self, event_loop: &mut EventLoop<WebSocketServer>, token: Token, events: EventSet ) {
        // for all readable events the token will be checked
        if events.is_readable() {
            // use the match to ensure we are working with a server socket connection
            match token {
                // matches the one specific server token
                SERVER_TOKEN => {
                    // determin the client socket safely 
                    let client_socket = match self.socket.accept() {
                        Err(e) => {
                            println!("Accept error {}",e );
                            return;
                        },
                        // as None is highly unlikely to ever occur crashing with unreachable is ok here
                        Ok(None) => unreachable!("Accept has returned none"),
                        // return the socket value as the client_socket to be used later on
                        Ok(Some((sock, addr))) => sock
                    };

                    self.token_counter += 1; //increase token counter
                    let new_token = Token(self.token_counter); // get the new token
                    self.clients.insert(new_token, WebsocketClient::new(client_socket)); // store in the hash map

                    event_loop.register(
                        &self.clients[&new_token].socket, 
                        new_token, 
                        EventSet::readable(), 
                        PollOpt::edge() | PollOpt::oneshot()
                    ).unwrap();
                },
                // match all other tokens
                token => {
                    // 
                    let mut client =self.clients.get_mut(&token).unwrap();
                    client.read();
                    event_loop.reregister(
                        &client.socket, 
                        token, 
                        client.interest, 
                        PollOpt::edge() | PollOpt::oneshot()
                    ).unwrap();
                }
            }
        }

        if events.is_writable() {
            let mut client = self.clients.get_mut(&token).unwrap();
            client.write();
            event_loop.reregister(
                &client.socket, 
                token, 
                client.interest, 
                PollOpt::edge() | PollOpt::oneshot()
            ).unwrap();
        }

    }
}

fn main() {
     // register the event loop to a socket with a TCP Listener
    let address = "0.0.0.0:10000".parse::<SocketAddr>().unwrap();
    let server_socket = TcpListener::bind(&address).unwrap();

    // setup the general event loop with the correct handler
    let mut event_loop = EventLoop::new().unwrap();
    let mut server = WebSocketServer {
        token_counter: 1,
        clients: HashMap::new(),
        socket: server_socket
    };

    event_loop.register(&server.socket, 
        SERVER_TOKEN, // sockets unique identifier
        EventSet::readable(), // what we expect to do with this registered socket (read, write, listen, everything)
        PollOpt::edge() // edge: notifications only on new data arriving, level: notifications when any data is there
    ).unwrap();

    event_loop.run(&mut server).unwrap(); // event loop with a mutable handler inside

}
