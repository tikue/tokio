use {Service};
use super::{Error, Frame, Transport};
use reactor::{Task, Tick};
use util::future::AwaitQueue;
use std::io;

/// A server `Task` that dispatches `Transport` messages to a `Service` using
/// protocol pipelining.
pub struct Server<S, T>
    where S: Service,
{
    run: bool,
    service: S,
    transport: T,
    in_flight: AwaitQueue<S::Fut>,
}


impl<S, T> Server<S, T>
    where S: Service,
{
    /// Create a new pipeline `Server` dispatcher with the given service and
    /// transport
    pub fn new(service: S, transport: T) -> io::Result<Server<S, T>> {
        Ok(Server {
            run: true,
            service: service,
            transport: transport,
            in_flight: try!(AwaitQueue::with_capacity(16)),
        })
    }
}

impl<S, T, E> Task for Server<S, T>
    where S: Service<Error = E>,
          T: Transport<In=S::Resp, Out=S::Req>,
          E: From<Error<T::Error>> + Send + 'static,
{
    fn tick(&mut self) -> io::Result<Tick> {
        trace!("pipeline::Server::tick");

        // The first action is always flushing the transport
        let mut flush = try!(self.transport.flush());

        // Handle completed responses
        while self.transport.is_writable() {
            trace!("pipeline transport is writable");

            // Try to get the next completed future
            match self.in_flight.poll() {
                Some(Ok(val)) => {
                    trace!("got in_flight value");
                    flush = try!(self.transport.write(Frame::Message(val)));
                }
                Some(Err(_)) => unimplemented!(),
                None => {
                    trace!("no response ready for write");
                    break;
                }
            }
        }

        // Process new requests as long as the server is accepting
        while self.run {
            trace!("pipeline trying to read transport");
            match self.transport.read() {
                Ok(Some(frame)) => {
                    match frame {
                        Frame::Message(req) => {
                            trace!("pipeline got request");
                            let resp = self.service.call(req);
                            self.in_flight.push(resp)
                        }
                        Frame::Done => {
                            trace!("received Frame::Done");
                            // At this point, we just return. This works
                            // because tick() will be called again and go
                            // through the read-cycle again.
                            self.run = false;
                            break;
                        }
                        Frame::Error(_) => {
                            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "An error occurred."));
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => panic!(e.to_string()),
            }
        }

        // Clean shutdown of the pipeline server can happen when
        //
        // 1. The server is done running, this is signaled by Transport::read()
        //    returning Frame::Done.
        //
        // 2. The transport is done writing all data to the socket, this is
        //    signaled by Transport::flush() returning Ok(Some(())).
        //
        // 3. There are no further responses to write to the transport.
        //
        // It is necessary to perfom these three checks in order to handle the
        // case where the client shuts down half the socket.
        //
        if !self.run && flush.is_some() && self.in_flight.is_empty() {
            return Ok(Tick::Final);
        }

        // Tick again later
        Ok(Tick::WouldBlock)
    }
}
