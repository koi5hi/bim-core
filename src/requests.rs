use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Barrier, RwLock};
use std::time::{Duration, Instant, SystemTime};

#[cfg(debug_assertions)]
use log::debug;

use rustls::{OwnedTrustAnchor, RootCertStore};
use url::Url;

pub trait GenericStream: Read + Write {}

impl<T: Read + Write> GenericStream for T {}

pub fn make_connection(
    address: &SocketAddr,
    url: &Url,
    ssl: bool,
) -> Result<Box<dyn GenericStream>, String> {
    let mut retry = 3;
    while retry > 0 {
        if let Ok(stream) = TcpStream::connect_timeout(&address, Duration::from_micros(3_000_000)) {
            #[cfg(debug_assertions)]
            debug!("TCP connected");

            let _r = stream.set_write_timeout(Some(Duration::from_secs(3)));
            let _r = stream.set_read_timeout(Some(Duration::from_secs(3)));
            if !ssl {
                return Ok(Box::new(stream));
            }

            let mut root_store = RootCertStore::empty();
            root_store.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(
                |ta| {
                    OwnedTrustAnchor::from_subject_spki_name_constraints(
                        ta.subject,
                        ta.spki,
                        ta.name_constraints,
                    )
                },
            ));
            let config = rustls::ClientConfig::builder()
                .with_safe_defaults()
                .with_root_certificates(root_store)
                .with_no_client_auth();

            let server_name = url.host_str().unwrap().try_into().unwrap();
            let conn = rustls::ClientConnection::new(Arc::new(config), server_name).unwrap();
            let tls = rustls::StreamOwned::new(conn, stream);

            #[cfg(debug_assertions)]
            debug!("SSL connected");

            return Ok(Box::new(tls));
        }

        retry -= 1;
    }
    return Err(String::from("连接失败"));
}

pub fn request_tcp_ping(address: &SocketAddr) -> Result<u128, String> {
    let now = Instant::now();
    let r = TcpStream::connect_timeout(&address, Duration::from_micros(1_000_000));
    let used = now.elapsed().as_micros();
    let used = match r {
        Ok(_) => used,
        Err(_e) => {
            #[cfg(debug_assertions)]
            debug!("Ping {_e}");

            1_000_000
        }
    };
    Ok(used)
}

pub fn request_http_download(
    address: SocketAddr,
    url: Url,
    connection_close: bool,
    ssl: bool,
    counter: Arc<RwLock<u128>>,
    barrier: Arc<Barrier>,
    flag: Arc<RwLock<bool>>,
    end: Arc<Barrier>,
) {
    let chunk_count = if connection_close {
        #[cfg(debug_assertions)]
        debug!("Enter connection close mode");

        15_000
    } else {
        50
    };
    let data_size = chunk_count * 1024 * 1024 as u128;
    let mut data_counter = data_size;
    let mut buffer = [0; 65536];

    let host_port = format!(
        "{}:{}",
        url.host_str().unwrap(),
        url.port_or_known_default().unwrap()
    );
    let path_str = url.path();

    let mut stream = match make_connection(&address, &url, ssl) {
        Ok(s) => s,
        Err(_) => {
            barrier.wait();
            end.wait();
            return;
        }
    };

    barrier.wait();
    while !*(flag.read().unwrap()) {
        if data_counter >= data_size {
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis();
            let path_query = format!(
                "{}?cors=true&r={}&ckSize={}&size={}",
                path_str, now, chunk_count, data_size
            );

            #[cfg(debug_assertions)]
            debug!("Download {path_query}");

            let request_head = format!(
                "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: bim/1.0\r\n\r\n",
                path_query, host_port,
            )
            .into_bytes();

            match stream.write_all(&request_head) {
                Ok(_) => {
                    data_counter = 0;
                }
                Err(_e) => {
                    #[cfg(debug_assertions)]
                    debug!("Download Error: {}", _e);

                    end.wait();
                    return;
                }
            }
        } else {
            match stream.read_exact(&mut buffer) {
                Ok(_) => {
                    {
                        let mut ct = counter.write().unwrap();
                        *ct += 65536;
                    }
                    data_counter += 65536;
                }
                Err(_e) => {
                    #[cfg(debug_assertions)]
                    debug!("Download Error: {}", _e);

                    end.wait();
                    return;
                }
            }
        }
    }
    end.wait();
}

pub fn request_http_upload(
    address: SocketAddr,
    url: Url,
    connection_close: bool,
    ssl: bool,
    counter: Arc<RwLock<u128>>,
    barrier: Arc<Barrier>,
    flag: Arc<RwLock<bool>>,
    end: Arc<Barrier>,
) {
    let chunk_count = if connection_close {
        #[cfg(debug_assertions)]
        debug!("Enter connection close mode");

        15_000
    } else {
        50
    };
    let data_size = chunk_count * 1024 * 1024 as u128;
    let mut data_counter = data_size;

    let host_port = format!(
        "{}:{}",
        url.host_str().unwrap(),
        url.port_or_known_default().unwrap()
    );
    let url_path = url.path();
    let request_chunk = "0123456789AaBbCcDdEeFfGgHhIiJjKkLlMmNnOoPpQqRrSsTtUuVvWwXxYyZz-="
        .repeat(1024)
        .into_bytes();

    let mut stream = match make_connection(&address, &url, ssl) {
        Ok(s) => s,
        Err(_) => {
            barrier.wait();
            end.wait();
            return;
        }
    };

    barrier.wait();
    while !*(flag.read().unwrap()) {
        if data_counter >= data_size {
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis();
            let path_query = format!("{}?r={}", url_path, now);

            #[cfg(debug_assertions)]
            debug!("Upload {path_query} size {data_size}");

            let request_head = format!(
                "POST {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: bim/1.0\r\nContent-Length: {}\r\n\r\n",
                path_query, host_port, data_size
            )
            .into_bytes();

            match stream.write_all(&request_head) {
                Ok(_) => {
                    {
                        let mut ct = counter.write().unwrap();
                        *ct += request_head.len() as u128;
                    }

                    data_counter = 0;
                }
                Err(_e) => {
                    #[cfg(debug_assertions)]
                    debug!("Upload Error: {}", _e);

                    end.wait();
                    return;
                }
            }
        } else {
            match stream.write_all(&request_chunk) {
                Ok(_) => {
                    {
                        let mut ct = counter.write().unwrap();
                        *ct += 65536;
                    }
                    data_counter += 65536;
                }
                Err(_e) => {
                    #[cfg(debug_assertions)]
                    debug!("Upload Error: {}", _e);

                    end.wait();
                    return;
                }
            }
        }
    }
    end.wait();
}
