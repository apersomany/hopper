use anyhow::{Result, bail};
use axum::{
    Router,
    extract::{ConnectInfo, Path, State},
    routing::get,
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Cursor},
    net::{Ipv6Addr, SocketAddr},
    sync::Arc,
};
use tokio::{
    fs,
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    select, signal, spawn,
};
use tracing::{info, warn};

const SEGMENT_BITS: u8 = 0x7F;
const CONTINUE_BIT: u8 = 0x80;

async fn read_varint<R: AsyncRead + Unpin>(reader: &mut R) -> Result<i32> {
    let mut tmp = CONTINUE_BIT;
    let mut val = 0;
    let mut pos = 0;
    while tmp & CONTINUE_BIT == CONTINUE_BIT {
        tmp = reader.read_u8().await?;
        val = val | ((tmp & SEGMENT_BITS) as i32) << pos;
        pos = pos + 7;
        if pos >= 32 {
            bail!("varint too long");
        }
    }
    Ok(val)
}

async fn read_string<R: AsyncRead + Unpin>(reader: &mut R) -> Result<String> {
    let mut buf = vec![0; read_varint(reader).await? as usize];
    reader.read_exact(&mut buf).await?;
    Ok(String::from_utf8(buf)?)
}

async fn write_varint<W: AsyncWrite + Unpin>(writer: &mut W, mut val: i32) -> Result<()> {
    loop {
        let tmp = val as u8 & SEGMENT_BITS;
        val = val >> 7;
        if val > 0 {
            writer.write_u8(tmp | CONTINUE_BIT).await?;
        } else {
            writer.write_u8(tmp).await?;
            break;
        }
    }
    Ok(())
}

async fn pipe<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(mut reader: R, mut writer: W) {
    let mut buf = [0; 1536];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                if writer.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = writer.shutdown().await;
}

async fn pipe_stream(a: TcpStream, b: TcpStream) -> Result<()> {
    a.set_nodelay(true)?;
    b.set_nodelay(true)?;
    let (a_reader, a_writer) = a.into_split();
    let (b_reader, b_writer) = b.into_split();
    select! {
        _ = pipe(a_reader, b_writer) => {},
        _ = pipe(b_reader, a_writer) => {},
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct Server {
    routes: DashMap<String, SocketAddr>,
    minecraft_proxy: SocketAddr,
    http_api_server: SocketAddr,
}

impl Default for Server {
    fn default() -> Self {
        info!("using default config");
        Self {
            routes: DashMap::new(),
            minecraft_proxy: SocketAddr::from((Ipv6Addr::UNSPECIFIED, 25565)),
            http_api_server: SocketAddr::from((Ipv6Addr::UNSPECIFIED, 80)),
        }
    }
}

impl Server {
    async fn new() -> Result<Arc<Server>> {
        Ok(Arc::new(match fs::read_to_string("config.json").await {
            Ok(routes) => {
                info!("reading config from config.json");
                serde_json::from_str(&routes)?
            }
            Err(error) => {
                if error.kind() == io::ErrorKind::NotFound {
                    warn!("config.json not found");
                    Self::default()
                } else {
                    bail!("error while reading config.json: {error}");
                }
            }
        }))
    }

    async fn proxy(self: Arc<Self>, mut edge: TcpStream, addr: SocketAddr) -> Result<()> {
        let mut packet = vec![0; read_varint(&mut edge).await? as usize];
        edge.read_exact(&mut packet).await?;
        let mut packet = Cursor::new(packet);
        if read_varint(&mut packet).await? != 0 {
            return Ok(());
        }
        let protocol = read_varint(&mut packet).await?;
        let hostname = read_string(&mut packet).await?;
        let origin = self
            .routes
            .get(hostname.as_str())
            .map(|origin| origin.clone());
        info!("new connection from {addr} to {hostname} using {protocol}");
        let Some(origin) = origin else {
            return Ok(());
        };
        let packet = packet.into_inner();
        let mut origin = TcpStream::connect(origin).await?;
        write_varint(&mut origin, packet.len() as i32).await?;
        origin.write_all(&packet).await?;
        pipe_stream(edge, origin).await?;
        Ok(())
    }

    async fn register(
        State(server): State<Arc<Server>>,
        Path(hostname): Path<String>,
        ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ) {
        let origin = SocketAddr::new(addr.ip(), 25565);
        info!("registered route {hostname} to {origin}");
        server.routes.insert(hostname, origin);
    }

    async fn shutdown(self: Arc<Self>) -> Result<()> {
        signal::ctrl_c().await?;
        info!("gracefully shutting down");
        fs::write("config.json", serde_json::to_vec_pretty(self.as_ref())?).await?;
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let server = Server::new().await.expect("failed to create server");
    select! {
        result = server.clone().shutdown() => result.expect("error while shutting down"),
        _ = async {
            let listener = TcpListener::bind(server.minecraft_proxy)
                .await
                .expect("failed to create listener");
            info!("minecraft proxy started on {:?}", server.minecraft_proxy);
            while let Ok((conn, addr)) = listener.accept().await {
                let server = server.clone();
                spawn(async move {
                    let Err(error) = server.proxy(conn, addr).await else {
                        return;
                    };
                    warn!("error while proxying connection from {addr}: {error}");
                });
            }
        } => {},
        _ = async {
            let listener = TcpListener::bind(server.http_api_server)
                .await
                .expect("failed to create listener");
            info!("http api server started on {:?}", server.http_api_server);
            let router = Router::new()
                .route("/register/{hostname}", get(Server::register))
                .with_state(server.clone())
                .into_make_service_with_connect_info::<SocketAddr>();
            axum::serve(listener, router).await.expect("error while serving http")
        } => {},
    };
}
