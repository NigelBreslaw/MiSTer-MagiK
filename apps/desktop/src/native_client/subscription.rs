//! One subscription, one cancellation handle and at most one reconnect.
use super::{AgentError, Client};
use std::{
    io::BufReader,
    net::{Shutdown, TcpStream},
    sync::{Arc, Mutex},
};
#[derive(Clone)]
pub struct Control(Arc<Mutex<Option<TcpStream>>>);
impl Control {
    pub fn shutdown(&self) {
        if let Some(stream) = self.0.lock().unwrap().take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}
pub struct Subscription {
    pub reader: BufReader<TcpStream>,
    pub request_id: String,
    pub control: Control,
    host: String,
    operation: &'static str,
    retried: bool,
}
impl Subscription {
    pub fn open(host: &str, operation: &'static str) -> Result<Self, AgentError> {
        let (request_id, reader) = Client::open(host)?.subscribe(operation)?;
        let control = Control(Arc::new(Mutex::new(Some(reader.get_ref().try_clone()?))));
        Ok(Self {
            reader,
            request_id,
            control,
            host: host.into(),
            operation,
            retried: false,
        })
    }
    pub fn reconnect(&mut self, error: &AgentError) -> Result<bool, AgentError> {
        if self.retried || !matches!(error, AgentError::Unreachable(_)) {
            return Ok(false);
        }
        {
            let control = self.control.0.lock().unwrap();
            let Some(old) = control.as_ref() else {
                return Ok(false);
            };
            let _ = old.shutdown(Shutdown::Both);
        }
        self.retried = true;
        let (request_id, reader) = Client::rediscover(&self.host)?.subscribe(self.operation)?;
        let mut control = self.control.0.lock().unwrap();
        if control.is_none() {
            let _ = reader.get_ref().shutdown(Shutdown::Both);
            return Ok(false);
        }
        *control = Some(reader.get_ref().try_clone()?);
        self.reader = reader;
        self.request_id = request_id;
        Ok(true)
    }
}
impl Drop for Subscription {
    fn drop(&mut self) {
        self.control.shutdown();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_subscription_never_rediscover_or_reconnects() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        let control = Control(Arc::new(Mutex::new(Some(stream.try_clone().unwrap()))));
        let mut session = Subscription {
            reader: BufReader::new(stream),
            control: control.clone(),
            request_id: "test".into(),
            host: "must-not-resolve".into(),
            operation: "telemetry-stream",
            retried: false,
        };
        control.shutdown();
        assert!(
            !session
                .reconnect(&AgentError::Unreachable("closed".into()))
                .unwrap()
        );
        assert!(!session.reconnect(&AgentError::Unauthorized).unwrap());
    }
}
