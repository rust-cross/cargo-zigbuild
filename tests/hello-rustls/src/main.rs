#[cfg(target_os = "macos")]
use libz_sys as _;

#[cfg(feature = "curl")]
extern crate curl;

#[tokio::main]
async fn main() {
    let response = reqwest::Client::new()
        .get("https://www.github.com")
        .send()
        .await
        .expect("send");
    println!("Response status {}", response.status());
}

#[cfg(test)]
mod test {
    #[test]
    fn it_works() {
        assert_eq!(1, 1);
    }
}
