use crate::node::iscsiutil::IscsiDisk;
use snafu::{ResultExt, Snafu};
use std::fmt::Debug;
use url::{ParseError, Url};

#[derive(Debug, Snafu)]
pub enum DevicePathError {
    #[snafu(display("Invalid URI \"{}\"", uri))]
    UriInvalid { source: ParseError, uri: String },
    #[snafu(display("Unsupported URI scheme \"{}\"", scheme))]
    UriSchemeUnsupported { scheme: String },
}

#[derive(Debug)]
pub enum DeviceType {
    IScsi,
    NBD,
    NVMF,
}

pub trait Attach {
    fn attach(&self) -> bool;
    fn www_id(&self);
}

#[derive(Debug)]
pub struct DevicePath {
    uri: String,
}

impl DevicePath {
    pub fn parse<T>(uri: &str) -> Result<impl Attach + Debug, DevicePathError> {
        let parsed = Url::parse(uri).context(UriInvalid {
            uri: uri.to_string(),
        })?;

        match parsed.scheme() {
            "iscsi" => IscsiDisk::new(parsed),
            scheme => Err(DevicePathError::UriSchemeUnsupported {
                scheme: scheme.into(),
            }),
        }
    }
}
