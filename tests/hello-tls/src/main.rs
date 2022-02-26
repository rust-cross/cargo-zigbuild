#[tokio::main]
async fn main() {
    let response = reqwest::Client::new()
        .post("http://www.baidu.com")
        .form(&[("one", "1")])
        .send()
        .await
        .expect("send");
    println!("Response status {}", response.status());
}