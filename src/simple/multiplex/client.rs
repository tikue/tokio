use BindClient;
use super::{RequestId, Multiplex};
use super::lift::{LiftBind, LiftTransport};
use simple::LiftProto;

use std::io;

use streaming::{self, Body, Message};
use streaming::multiplex::StreamingMultiplex;
use tokio_core::reactor::Handle;
use tokio_service::Service;
use futures::{stream, Stream, Sink, Future, IntoFuture, Poll};
use util::client_proxy;

type MyStream<E> = stream::Empty<(), E>;

/// An multiplexed client protocol.
///
/// The `T` parameter is used for the I/O object used to communicate, which is
/// supplied in `bind_transport`.
///
/// For simple protocols, the `Self` type is often a unit struct. In more
/// advanced cases, `Self` may contain configuration information that is used
/// for setting up the transport in `bind_transport`.
pub trait ClientProto<T: 'static>: 'static {
    /// Request messages.
    type Request: 'static;

    /// Response messages.
    type Response: 'static;

    /// The message transport, which usually take `T` as a parameter.
    ///
    /// An easy way to build a transport is to use `tokio_core::io::Framed`
    /// together with a `Codec`; in that case, the transport type is
    /// `Framed<T, YourCodec>`. See the crate docs for an example.
    type Transport: 'static +
        Stream<Item = (RequestId, Self::Response), Error = io::Error> +
        Sink<SinkItem = (RequestId, Self::Request), SinkError = io::Error>;

    /// A future for initializing a transport from an I/O object.
    ///
    /// In simple cases, `Result<Self::Transport, Self::Error>` often suffices.
    type BindTransport: IntoFuture<Item = Self::Transport, Error = io::Error>;

    /// Build a transport from the given I/O object, using `self` for any
    /// configuration.
    ///
    /// An easy way to build a transport is to use `tokio_core::io::Framed`
    /// together with a `Codec`; in that case, `bind_transport` is just
    /// `io.framed(YourCodec)`. See the crate docs for an example.
    fn bind_transport(&self, io: T) -> Self::BindTransport;
}

impl<T: 'static, P: ClientProto<T>> BindClient<Multiplex, T> for P {
    type ServiceRequest = P::Request;
    type ServiceResponse = P::Response;
    type ServiceError = io::Error;

    type BindClient = ClientService<P::Request, P::Response>;

    fn bind_client(&self, handle: &Handle, io: T) -> Self::BindClient {
        ClientService {
            inner: BindClient::<StreamingMultiplex<MyStream<io::Error>>, T>::bind_client(
                LiftProto::from_ref(self), handle, io
            )
        }
    }
}

impl<T, P> streaming::multiplex::ClientProto<T> for LiftProto<P> where
    T: 'static, P: ClientProto<T>
{
    type Request = P::Request;
    type RequestBody = ();

    type Response = P::Response;
    type ResponseBody = ();

    type Error = io::Error;

    type Transport = LiftTransport<P::Transport, io::Error>;
    type BindTransport = LiftBind<T, <P::BindTransport as IntoFuture>::Future, io::Error>;

    fn bind_transport(&self, io: T) -> Self::BindTransport {
        LiftBind::lift(ClientProto::bind_transport(self.lower(), io).into_future())
    }
}

type ClientProxy<Request, Response> =
    client_proxy::ClientProxy<Message<Request, MyStream<io::Error>>,
                              Message<Response, Body<(), io::Error>>,
                              io::Error>;

/// Client `Service` for simple multiplex protocols
pub struct ClientService<Request, Response> {
    inner: ClientProxy<Request, Response>,
}

impl<Request, Response> Service for ClientService<Request, Response> {
    type Request = Request;
    type Response = Response;
    type Error = io::Error;
    type Future = ClientFuture<Request, Response>;

    fn call(&self, req: Request) -> Self::Future {
        ClientFuture {
            inner: self.inner.call(Message::WithoutBody(req))
        }
    }
}

impl<Request, Response> Clone for ClientService<Request, Response> {
    fn clone(&self) -> Self {
        ClientService {
            inner: self.inner.clone(),
        }
    }
}

pub struct ClientFuture<Request, Response> {
    inner: <ClientProxy<Request, Response> as Service>::Future
}

impl<Request, Response> Future for ClientFuture<Request, Response> {
    type Item = Response;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match try_ready!(self.inner.poll()) {
            Message::WithoutBody(msg) => Ok(msg.into()),
            Message::WithBody(..) => panic!("bodies not supported"),
        }
    }
}
