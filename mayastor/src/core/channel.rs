use spdk_sys::{spdk_io_channel, spdk_put_io_channel};

#[derive(Debug, Clone)]
pub struct IoChannel(*mut spdk_io_channel);

impl Drop for IoChannel {
    fn drop(&mut self) {
        debug!("closing io channel {:p}", self.0);
        unsafe { spdk_put_io_channel(self.0) }
    }
}
impl IoChannel {
    pub fn from_null_checked(ch: *mut spdk_io_channel) -> Option<IoChannel> {
        if ch.is_null() {
            None
        } else {
            Some(IoChannel(ch))
        }
    }

    pub fn as_ptr(&self) -> *mut spdk_io_channel {
        self.0
    }
}
