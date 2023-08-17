#[tokio::main]
async fn main() {
    let response = reqwest::Client::new()
        .get("https://www.github.com")
        .send()
        .await
        .expect("send");
    println!("Response status {}", response.status());
}
