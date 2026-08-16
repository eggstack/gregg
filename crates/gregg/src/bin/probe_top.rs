use std::time::Duration;

fn main() {
    let host = std::env::var("PROBE_HOST").unwrap_or_else(|_| "192.168.182.143".to_string());
    let port: u16 = std::env::var("PROBE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(11310);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .unwrap();
    rt.block_on(async move {
        eprintln!("tokio TcpStream::connect");
        match tokio::net::TcpStream::connect((host.as_str(), port)).await {
            Ok(_) => eprintln!("ok"),
            Err(e) => eprintln!("err: {e}"),
        }
        eprintln!("std TcpStream::connect");
        let conn = std::net::TcpStream::connect_timeout(
            &format!("{host}:{port}").parse().unwrap(),
            Duration::from_secs(5),
        );
        match conn {
            Ok(_) => eprintln!("std ok"),
            Err(e) => eprintln!("std err: {e}"),
        }
    });
}
