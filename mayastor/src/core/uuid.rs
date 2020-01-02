use spdk_sys::spdk_uuid;
/// Muuid provides several From trait implementations for the raw spdk UUIDs
/// It depends on largely, on the bdev, if you can set the uuid in a nicer way
#[derive(Debug)]
pub struct Uuid(pub(crate) *const spdk_uuid);

impl Uuid {
    /// For some reason the uuid is a union
    pub fn as_bytes(&self) -> [u8; 16] {
        unsafe { (*self.0).u.raw }
    }
}
