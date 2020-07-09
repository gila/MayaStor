use crate::{
    core::Bdev,
    grpc::GrpcResult,
    nexus_uri::{bdev_create, NexusBdevError},
};

use crate::{
    bdev::Uri,
    target::{iscsi, Side},
};
use rpc::{
    mayastor::{Null, PublishNexusRequest},
    service::{bdev_rpc_server::BdevRpc, BdevUri, Bdevs, CreateReply},
};
use tonic::{Request, Response, Status};
use tracing::instrument;

impl From<NexusBdevError> for tonic::Status {
    fn from(e: NexusBdevError) -> Self {
        match e {
            NexusBdevError::UrlParseError {
                ..
            } => Status::invalid_argument(e.to_string()),
            NexusBdevError::UriSchemeUnsupported {
                ..
            } => Status::invalid_argument(e.to_string()),
            NexusBdevError::UriInvalid {
                ..
            } => Status::invalid_argument(e.to_string()),
            e => Status::internal(e.to_string()),
        }
    }
}
impl From<Bdev> for rpc::service::Bdev {
    fn from(b: Bdev) -> Self {
        Self {
            name: b.name(),
            uuid: b.uuid_as_string(),
            num_blocks: b.num_blocks(),
            blk_size: b.block_len(),
            claimed: b.is_claimed(),
            claimed_by: b.claimed_by().unwrap_or_else(|| "Orphaned".into()),
        }
    }
}

#[derive(Debug)]
pub struct BdevSvc {}

#[tonic::async_trait]
impl BdevRpc for BdevSvc {
    #[instrument(level = "debug", err)]
    async fn list(&self, _request: Request<Null>) -> GrpcResult<Bdevs> {
        let mut list: Vec<rpc::service::Bdev> = Vec::new();
        if let Some(bdev) = Bdev::bdev_first() {
            bdev.into_iter().for_each(|bdev| list.push(bdev.into()))
        }

        Ok(Response::new(Bdevs {
            bdevs: list,
        }))
    }

    #[instrument(level = "debug", err)]
    async fn create(
        &self,
        request: Request<BdevUri>,
    ) -> GrpcResult<CreateReply> {
        let uri = request.into_inner().uri;
        let bdev = locally! { async move { bdev_create(&uri).await } };

        Ok(Response::new(CreateReply {
            name: bdev,
        }))
    }

    #[instrument(level = "debug", err)]
    async fn share(
        &self,
        request: Request<PublishNexusRequest>,
    ) -> GrpcResult<BdevUri> {
        let parsed = Uri::parse(&request.into_inner().uuid)?.get_name();
        match Bdev::lookup_by_name(&parsed) {
            Some(bdev) => iscsi::share(&bdev.uuid_as_string(), &bdev, Side::Nexus)
                .map_err(|_e| Status::internal("stuk")).map(|_share|{
               Response::new(BdevUri {
                    uri: "congrats, its shared somewhere, see the logs, use kubectl or something!".into()
                })
            }),
            None => Err(Status::not_found(parsed))
        }
    }
}
