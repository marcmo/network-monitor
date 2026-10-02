use std::{
    future::Future,
    io,
    net::{SocketAddr, ToSocketAddrs},
    pin::Pin,
    thread,
    time::Duration,
};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use crate::{
    config::Config,
    model::{ProbeKind, ProbeOutcome},
    scheduler::{ProbeRunner, ProbeSpec},
};

#[derive(Debug, Error)]
pub enum ProbeSetupError {
    #[error("cannot start system DNS worker: {0}")]
    Dns(#[from] io::Error),
    #[error("cannot configure HTTPS probe: {0}")]
    Http(#[from] reqwest::Error),
}

#[derive(Debug, Error)]
enum ResolverError {
    #[error("system DNS worker is still busy")]
    Busy,
    #[error("system DNS worker stopped")]
    Stopped,
    #[error("system DNS failed: {0}")]
    Lookup(#[from] io::Error),
    #[error("DNS returned no IP addresses")]
    Empty,
}

impl ResolverError {
    fn is_unavailable(&self) -> bool {
        matches!(self, Self::Busy | Self::Stopped)
    }
}

struct DnsRequest {
    name: String,
    reply: oneshot::Sender<io::Result<Vec<SocketAddr>>>,
}

#[derive(Clone)]
struct SystemResolver {
    sender: mpsc::Sender<DnsRequest>,
}

impl SystemResolver {
    fn new() -> io::Result<Self> {
        let (sender, mut receiver) = mpsc::channel::<DnsRequest>(2);
        thread::Builder::new()
            .name("system-dns".into())
            .spawn(move || {
                // One worker bounds uncancellable getaddrinfo work even if the system resolver stalls.
                while let Some(request) = receiver.blocking_recv() {
                    if request.reply.is_closed() {
                        continue;
                    }
                    let result = (request.name.as_str(), 0)
                        .to_socket_addrs()
                        .map(|addresses| addresses.take(32).collect());
                    let _ = request.reply.send(result);
                }
            })?;
        Ok(Self { sender })
    }

    async fn lookup(&self, name: String) -> Result<Vec<SocketAddr>, ResolverError> {
        let (reply, receiver) = oneshot::channel();
        self.sender
            .try_send(DnsRequest { name, reply })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => ResolverError::Busy,
                mpsc::error::TrySendError::Closed(_) => ResolverError::Stopped,
            })?;
        let addresses = receiver.await.map_err(|_| ResolverError::Stopped)??;
        if addresses.is_empty() {
            return Err(ResolverError::Empty);
        }
        Ok(addresses)
    }
}

impl Resolve for SystemResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let resolver = self.clone();
        let name = name.as_str().to_owned();
        Box::pin(async move {
            let addresses = resolver.lookup(name).await?;
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

#[derive(Clone)]
pub struct NetworkProbes {
    resolver: SystemResolver,
    client: reqwest::Client,
}

impl NetworkProbes {
    pub fn new(config: &Config) -> Result<Self, ProbeSetupError> {
        let resolver = SystemResolver::new()?;
        let client = reqwest::Client::builder()
            .dns_resolver2(resolver.clone())
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_millis(config.https_timeout_ms))
            .timeout(Duration::from_millis(config.https_timeout_ms))
            .pool_max_idle_per_host(1)
            .pool_idle_timeout(Duration::from_secs(40))
            .http1_only()
            .user_agent(concat!("network-monitor/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { resolver, client })
    }
}

impl ProbeRunner for NetworkProbes {
    fn probe(&self, spec: ProbeSpec) -> Pin<Box<dyn Future<Output = ProbeOutcome> + Send>> {
        let probes = self.clone();
        Box::pin(async move {
            match spec.kind {
                ProbeKind::Tcp => {
                    let address = match spec.target.parse::<SocketAddr>() {
                        Ok(address) => address,
                        Err(error) => return ProbeOutcome::Unavailable(error.to_string()),
                    };
                    match tokio::net::TcpStream::connect(address).await {
                        Ok(_) => ProbeOutcome::Success,
                        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                            ProbeOutcome::Refused
                        }
                        Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                            ProbeOutcome::Timeout
                        }
                        Err(error) => ProbeOutcome::NetworkError(error.to_string()),
                    }
                }
                ProbeKind::Dns => match probes.resolver.lookup(spec.target).await {
                    Ok(_) => ProbeOutcome::Success,
                    Err(error) if error.is_unavailable() => {
                        ProbeOutcome::Unavailable(error.to_string())
                    }
                    Err(error) => ProbeOutcome::DnsError(error.to_string()),
                },
                ProbeKind::Https => match probes.client.head(spec.target).send().await {
                    Ok(response) if response.status().is_success() => ProbeOutcome::Success,
                    Ok(response) => ProbeOutcome::HttpStatus(response.status().as_u16()),
                    Err(error) if error.is_timeout() => ProbeOutcome::Timeout,
                    Err(error) => https_error_outcome(&error),
                },
            }
        })
    }
}

fn https_error_outcome(error: &reqwest::Error) -> ProbeOutcome {
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = cause {
        if let Some(resolver_error) = current.downcast_ref::<ResolverError>() {
            if resolver_error.is_unavailable() {
                return ProbeOutcome::Unavailable(resolver_error.to_string());
            }
        }
        cause = current.source();
    }
    ProbeOutcome::TlsOrHttpError(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn saturated_dns_service_is_unavailable_for_https_not_a_network_failure() {
        let (sender, _requests) = mpsc::channel(1);
        let (reply, _response) = oneshot::channel();
        sender
            .try_send(DnsRequest {
                name: "localhost".into(),
                reply,
            })
            .unwrap();
        let resolver = SystemResolver { sender };
        let client = reqwest::Client::builder()
            .dns_resolver2(resolver.clone())
            .no_proxy()
            .build()
            .unwrap();
        let probes = NetworkProbes { resolver, client };
        let outcome = probes
            .probe(ProbeSpec {
                kind: ProbeKind::Https,
                target: "https://localhost".into(),
                interval_ms: 30000,
                timeout_ms: 3000,
            })
            .await;
        assert!(
            matches!(outcome, ProbeOutcome::Unavailable(_)),
            "{outcome:?}"
        );
    }
}
