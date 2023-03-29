use clap::Parser;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Listen on this address
    #[arg(env, long, default_value = "0.0.0.0", value_name = "HOST")]
    pub listen_host: String,

    /// Listen on this port
    #[arg(env, long, default_value_t = 3000, value_name = "PORT")]
    pub listen_port: u16,

    /// Deployment to scale
    #[arg(env = "DEPLOYMENT", short = 'd', long, value_name = "NAME")]
    pub deployment: String,

    /// Service to proxy to
    #[arg(env = "SERVICE", short = 's', long, value_name = "NAME")]
    pub service: String,

    /// Port name of the service
    #[arg(env = "PORT", long, value_name = "NAME")]
    pub service_port: Option<String>,

    /// Should sero inject itself into the services Endpoints?
    #[arg(env, short = 'i', long)]
    pub inject: bool,
}
