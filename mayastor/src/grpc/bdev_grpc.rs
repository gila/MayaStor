use tonic::{Request, Response, Status};
use tracing::instrument;

use std::convert::TryFrom;
use url::Url;

use rpc::mayastor::{
    bdev_rpc_server::BdevRpc,
    Bdev as RpcBdev,
    BdevShareReply,
    BdevShareRequest,
    BdevUri,
    Bdevs,
    CreateReply,
    Null,
};

use crate::{
    core::{Bdev, Reactors, Share},
    grpc::GrpcResult,
    jsonrpc::{
        jsonrpc_register,
        print_error_chain,
        Code,
        JsonRpcError,
        RpcErrorCode,
    },
    nexus_uri::{bdev_create, bdev_destroy, NexusBdevError},
};
use futures::FutureExt;

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

impl From<NexusBdevError> for JsonRpcError {
    fn from(e: NexusBdevError) -> Self {
        JsonRpcError {
            code: e.rpc_error_code(),
            message: print_error_chain(&e),
        }
    }
}

impl RpcErrorCode for NexusBdevError {
    fn rpc_error_code(&self) -> Code {
        match self {
            NexusBdevError::UrlParseError {
                ..
            } => Code::InvalidParams,
            NexusBdevError::BdevNoUri {
                ..
            } => Code::InvalidParams,
            NexusBdevError::UriSchemeUnsupported {
                ..
            } => Code::InvalidParams,
            NexusBdevError::UriInvalid {
                ..
            } => Code::InvalidParams,
            NexusBdevError::BoolParamParseError {
                ..
            } => Code::InvalidParams,
            NexusBdevError::IntParamParseError {
                ..
            } => Code::InvalidParams,
            NexusBdevError::UuidParamParseError {
                ..
            } => Code::InvalidParams,
            NexusBdevError::BdevExists {
                ..
            } => Code::AlreadyExists,
            NexusBdevError::BdevNotFound {
                ..
            } => Code::NotFound,
            NexusBdevError::InvalidParams {
                ..
            } => Code::InvalidParams,
            NexusBdevError::CreateBdev {
                ..
            } => Code::InternalError,
            NexusBdevError::DestroyBdev {
                ..
            } => Code::InternalError,
            NexusBdevError::CancelBdev {
                ..
            } => Code::InternalError,
        }
    }
}

impl From<Bdev> for RpcBdev {
    fn from(b: Bdev) -> Self {
        Self {
            name: b.name(),
            uuid: b.uuid_as_string(),
            num_blocks: b.num_blocks(),
            blk_size: b.block_len(),
            claimed: b.is_claimed(),
            claimed_by: b.claimed_by().unwrap_or_else(|| "Orphaned".into()),
            aliases: b.aliases().join(","),
            product_name: b.product_name(),
            share_uri: b.share_uri().unwrap_or_else(|| "".into()),
            uri: Url::try_from(b).map_or("".into(), |u| u.to_string()),
        }
    }
}

#[derive(Debug)]
pub struct BdevSvc;

pub fn bdev_methods() {
    jsonrpc_register("bdev_create", |args: BdevUri| {
        async move { bdev_create(&args.uri).await }.boxed_local()
    });

    jsonrpc_register("bdev_destroy", |args: BdevUri| {
        async move { bdev_destroy(&args.uri).await }.boxed_local()
    });

    jsonrpc_register("bdev_share", |args: BdevShareRequest| {
        let name = args.name.clone();
        async move {
            if Bdev::lookup_by_name(&name).is_none() {
                return Err(JsonRpcError::not_found(&name));
            }

            if args.proto != "iscsi" && args.proto != "nvmf" {
                return Err(JsonRpcError::invalid_argument(args.proto));
            }

            match args.proto.as_str() {
                "nvmf" => Reactors::master().spawn_local(async move {
                    let bdev = Bdev::lookup_by_name(&name).unwrap();
                    bdev.share_nvmf()
                        .await
                        .map_err(|e| JsonRpcError::internal(e.to_string()))
                }),

                "iscsi" => Reactors::master().spawn_local(async move {
                    let bdev = Bdev::lookup_by_name(&name).unwrap();
                    bdev.share_iscsi()
                        .await
                        .map_err(|e| JsonRpcError::internal(e.to_string()))
                }),
                _ => unreachable!(),
            }
            .await
            .map(|share| {
                let bdev = Bdev::lookup_by_name(&args.name.clone()).unwrap();
                BdevShareReply {
                    uri: bdev.share_uri().unwrap_or(share),
                }
            })
        }
        .boxed_local()
    });

    jsonrpc_register("bdev_unshare", |args: BdevShareRequest| {
        async move {
            let bdev = Bdev::lookup_by_name(&args.name).unwrap();
            bdev.unshare()
                .await
                .map_err(|e| JsonRpcError::internal(e.to_string()))
        }
        .boxed_local()
    });
}

#[tonic::async_trait]
impl BdevRpc for BdevSvc {
    #[instrument(level = "debug", err)]
    async fn list(&self, _request: Request<Null>) -> GrpcResult<Bdevs> {
        let mut list: Vec<RpcBdev> = Vec::new();
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
    ) -> Result<Response<CreateReply>, Status> {
        let result: String = jsonrpc::call(
            "/var/tmp/mayastor",
            "bdev_create",
            Some(request.into_inner()),
        )
        .await?;
        Ok(Response::new(CreateReply {
            name: result,
        }))
    }

    #[instrument(level = "debug", err)]
    async fn destroy(&self, request: Request<BdevUri>) -> GrpcResult<Null> {
        jsonrpc::call(
            "/var/tmp/mayastor",
            "bdev_destroy",
            Some(request.into_inner()),
        )
        .await?;

        Ok(Response::new(Null {}))
    }

    #[instrument(level = "debug", err)]
    async fn share(
        &self,
        request: Request<BdevShareRequest>,
    ) -> GrpcResult<BdevShareReply> {
        let uri: String = jsonrpc::call(
            "/var/tmp/mayastor",
            "bdev_share",
            Some(request.into_inner()),
        )
        .await?;

        Ok(Response::new(BdevShareReply {
            uri,
        }))
    }

    #[instrument(level = "debug", err)]
    async fn unshare(&self, request: Request<CreateReply>) -> GrpcResult<Null> {
        jsonrpc::call(
            "/var/tmp/mayastor",
            "bdev_unshare",
            Some(request.into_inner()),
        )
        .await?;

        Ok(Response::new(Null {}))
    }
}
