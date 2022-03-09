#[tokio::main]
async fn main() {
    let response = reqwest::Client::new()
        .get("http://www.baidu.com")
        .send()
        .await
        .expect("send");
    println!("Response status {}", response.status());
}
