use clap::Parser;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Listen on this address
    #[arg(
        env,
        short,
        long,
        value_name = "HOST:PORT",
        default_value = "0.0.0.0:3000"
    )]
    pub listen_address: String,

    /// Proxy connections to this address
    #[arg(env, short, long, value_name = "HOST:PORT")]
    pub backend_address: String,

    /// Deployment to scale
    #[arg(env, short = 'd', long, value_name = "NAME")]
    pub target_deploy: String,

    /// Service to watch for endpoints
    #[arg(env, short = 's', long, value_name = "NAME")]
    pub target_svc: String,
}
