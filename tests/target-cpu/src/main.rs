use std::io;

fn main() {
    zstd::stream::copy_decode(io::stdin(), io::stdout()).unwrap();
}
