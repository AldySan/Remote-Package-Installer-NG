use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.pkg");
    std::fs::write(&path, vec![7u8; 4096]).unwrap();
    let server = rpi_ng_core::fileserver::FileServer::start(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let id = server.register(path, Some("game.pkg".into()));
    let url = server.url_for(id).unwrap();
    println!("url: {url}");

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client.head(&url).send().await.unwrap();
    println!("HEAD status: {}", resp.status());
    let resp = client.get(&url).send().await.unwrap();
    println!("GET status: {}", resp.status());
    let b = resp.bytes().await.unwrap();
    println!("GET body len: {}", b.len());
    let resp = client
        .get(&url)
        .header("Range", "bytes=100-199")
        .send()
        .await
        .unwrap();
    println!("RANGE status: {}", resp.status());
}
