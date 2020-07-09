//!
//! methods to directly interact with the bdev layer

use crate::context::Context;
use clap::{App, AppSettings, Arg, ArgMatches, SubCommand};
use colored_json::prelude::*;
use rpc::{
    mayastor::{Null, PublishNexusRequest},
    service::BdevUri,
};
use tonic::Status;

pub async fn handler(
    ctx: Context,
    matches: &ArgMatches<'_>,
) -> Result<(), Status> {
    match matches.subcommand() {
        ("create", Some(args)) => create(ctx, args).await,
        ("list", Some(args)) => list(ctx, args).await,
        ("share", Some(args)) => share(ctx, args).await,
        (cmd, _) => {
            Err(Status::not_found(format!("command {} does not exist", cmd)))
        }
    }
}
pub fn subcommands<'a, 'b>() -> App<'a, 'b> {
    let create = SubCommand::with_name("create")
        .about("Create a new bdev by specifying a URI")
        .arg(Arg::with_name("uri").required(true).index(1));

    let share = SubCommand::with_name("share")
        .about("Create a new bdev by specifying a URI")
        .arg(Arg::with_name("uri").required(true).index(1));

    let list = SubCommand::with_name("list").about("List all bdevs");
    SubCommand::with_name("bdev")
        .settings(&[
            AppSettings::SubcommandRequiredElseHelp,
            AppSettings::ColoredHelp,
            AppSettings::ColorAlways,
        ])
        .about("Block device management")
        .subcommand(list)
        .subcommand(create)
        .subcommand(share)
}

async fn list(mut ctx: Context, _args: &ArgMatches<'_>) -> Result<(), Status> {
    let bdevs = ctx.bdev.list(Null {}).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&bdevs.into_inner())
            .unwrap()
            .to_colored_json_auto()
            .unwrap()
    );

    Ok(())
}

async fn create(mut ctx: Context, args: &ArgMatches<'_>) -> Result<(), Status> {
    let uri = args.value_of("uri").unwrap().to_owned();
    let response = ctx
        .bdev
        .create(BdevUri {
            uri,
        })
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&response.into_inner())
            .unwrap()
            .to_colored_json_auto()
            .unwrap()
    );
    Ok(())
}

async fn share(mut ctx: Context, args: &ArgMatches<'_>) -> Result<(), Status> {
    let uri = args.value_of("uri").unwrap().to_owned();
    let response = ctx
        .bdev
        .share(PublishNexusRequest {
            uuid: uri,
            key: "".to_string(),
            share: 2,
        })
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&response.into_inner())
            .unwrap()
            .to_colored_json_auto()
            .unwrap()
    );
    Ok(())
}
