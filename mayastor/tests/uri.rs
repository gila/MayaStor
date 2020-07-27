use mayastor::bdev::Uri;

#[test]
fn uri_parse_aio() {
    let dev = Uri::parse("aio:///dev/sdb").unwrap();
    dbg!(dev);
}
