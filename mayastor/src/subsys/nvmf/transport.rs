use std::{
    env,
    ffi::CString,
    fmt::{Debug, Display},
    net::Ipv4Addr,
    ptr::copy_nonoverlapping,
};

use futures::channel::oneshot;
use nix::errno::Errno;
use once_cell::sync::Lazy;
use serde::export::Formatter;

use spdk_sys::{
    spdk_nvme_transport_id,
    spdk_nvmf_tgt_add_transport,
    spdk_nvmf_transport_create,
    SPDK_NVME_TRANSPORT_TCP,
    SPDK_NVMF_ADRFAM_IPV4,
    SPDK_NVMF_TRSVCID_MAX_LEN,
};

use crate::{
    ffihelper::{
        cb_arg,
        done_errno_cb,
        AsStr,
        ErrnoResult,
        FfiResult,
        IntoCString,
    },
    subsys::{
        nvmf::{Error, NVMF_TGT},
        Config,
    },
};
use std::ops::{Deref, DerefMut};

static TCP_TRANSPORT: Lazy<CString> =
    Lazy::new(|| CString::new("TCP").unwrap());

pub async fn add_tcp_transport() -> Result<(), Error> {
    let cfg = Config::get();
    let mut opts = cfg.nvmf_tcp_tgt_conf.opts.clone().into();
    let transport = unsafe {
        spdk_nvmf_transport_create(TCP_TRANSPORT.as_ptr(), &mut opts)
    };

    transport.to_result(|_| Error::Transport {
        source: Errno::UnknownErrno,
        msg: "failed to create transport".into(),
    })?;

    let (s, r) = oneshot::channel::<ErrnoResult<()>>();
    unsafe {
        NVMF_TGT.with(|t| {
            spdk_nvmf_tgt_add_transport(
                t.borrow().tgt.as_ptr(),
                transport,
                Some(done_errno_cb),
                cb_arg(s),
            );
        })
    };

    let _result = r.await.unwrap();

    debug!("Added TCP nvmf transport");
    Ok(())
}

pub struct TransportID(pub(crate) spdk_nvme_transport_id);
impl Deref for TransportID {
    type Target = spdk_nvme_transport_id;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for TransportID {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl TransportID {
    pub fn new(port: u16) -> Self {
        let address = get_ipv4_address().unwrap();

        let mut trid: spdk_nvme_transport_id = Default::default();
        trid.trtype = SPDK_NVME_TRANSPORT_TCP;
        trid.adrfam = SPDK_NVMF_ADRFAM_IPV4;

        let c_addr = address.into_cstring();
        let port = format!("{}", port);

        assert!(port.len() < SPDK_NVMF_TRSVCID_MAX_LEN as usize);
        let c_port = port.into_cstring();

        unsafe {
            copy_nonoverlapping(
                TCP_TRANSPORT.as_ptr(),
                &mut trid.trstring[0],
                trid.trstring.len(),
            );
            copy_nonoverlapping(
                c_addr.as_ptr(),
                &mut trid.traddr[0],
                c_addr.as_bytes().len(),
            );
            copy_nonoverlapping(
                c_port.as_ptr(),
                &mut trid.trsvcid[0],
                c_port.as_bytes().len(),
            );
        }
        Self(trid)
    }

    pub fn as_ptr(&self) -> *mut spdk_nvme_transport_id {
        &self.0 as *const _ as *mut spdk_nvme_transport_id
    }
}

impl Display for TransportID {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "nvmf://{}:{}",
            self.0.traddr.as_str(),
            self.0.trsvcid.as_str()
        )
    }
}

impl Debug for TransportID {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transport ID")
            .field("trtype", &self.0.trtype)
            .field("trstring", &self.0.trstring.as_str().to_string())
            .field("traddr", &self.0.traddr.as_str().to_string())
            .field("trsvcid", &self.0.trsvcid.as_str().to_string())
            .finish()
    }
}
pub(crate) fn get_ipv4_address() -> Result<String, Error> {
    let address = match env::var("MY_POD_IP") {
        Ok(val) => {
            if val.parse::<Ipv4Addr>().is_ok() {
                Ok(val)
            } else {
                Err(Error::CreateTarget {
                    msg: "Invalid IPv4 address".into(),
                })
            }
        }
        Err(_) => Ok("127.0.0.1".to_owned()),
    }?;

    Ok(address)
}
