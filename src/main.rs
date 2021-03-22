use anyhow::{anyhow, bail, Context, Result};
use clap::{App, Arg, SubCommand};
use serde::Deserialize;
use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, Deserialize)]
struct Resource {
    username: Option<String>,
    server: HashMap<String, EnvironmentDef<ServerDef>>,
    resource: HashMap<String, EnvironmentDef<ResourceDef>>,
}

#[derive(Clone, Debug, Deserialize)]
struct EnvironmentDef<D>(HashMap<String, D>);

#[derive(Clone, Debug, Deserialize)]
struct ServerDef {
    ip: Option<String>,
    name: Option<String>,
    jump: Option<String>,
    proxy: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
struct ResourceDef {
    server: String,
    at: String,
    port: u16,
}

fn home() -> PathBuf {
    // even though it's deprecated, it's still a relatively good/cheaper option,
    // at least better than just getting $HOME directly ..
    #[allow(deprecated)]
    std::env::home_dir().expect("HOME directory")
}

fn ssh_dir() -> PathBuf {
    let mut path = home();
    path.push(".ssh");
    path
}

fn machlist_local() -> PathBuf {
    let mut path = home();
    path.push(".machlist/resources.toml");
    path
}

fn user_host(user: Option<&str>, host: &str) -> String {
    match user {
        Some(u) => format!("{}@{}", u, host),
        None => host.to_string(),
    }
}

/// Get the resources file
///
/// If specified (Some), then we only this file directly,
/// but when unspecified (None), we look at a local file called ./machlist-resources.toml
/// and then ~/.machlist/resources.toml
fn parse_resources<P: AsRef<Path>>(file: P) -> Result<Resource> {
    let file = file.as_ref();
    let content = std::fs::read_to_string(file)
        .with_context(|| format!("Failed to parse resource file {}", file.display()))?;

    let values: Resource = toml::de::from_str(&content)?;
    Ok(values)
}

impl Resource {
    pub fn get_target_env(&self, target_env: &str) -> Result<&EnvironmentDef<ServerDef>> {
        self.server
            .get(target_env)
            .ok_or_else(|| anyhow!("cannot find specified target environment in servers"))
    }

    pub fn get_target_env_resources(
        &self,
        target_env: &str,
    ) -> Result<&EnvironmentDef<ResourceDef>> {
        self.resource.get(target_env).ok_or(anyhow!(
            "cannot find specified target environment in resources"
        ))
    }

    pub fn get_username(&self) -> Result<Option<String>> {
        match &self.username {
            None => Ok(None),
            Some(u) => {
                if let Some(env_name) = u.strip_prefix("env:") {
                    Ok(Some(std::env::var(env_name).with_context(|| {
                        format!("Cannot find environment variable {}", env_name)
                    })?))
                } else {
                    Ok(Some(u.clone()))
                }
            }
        }
    }
}

impl EnvironmentDef<ServerDef> {
    pub fn get_machine(&self, machine_name: &str) -> Result<&ServerDef> {
        self.0
            .get(machine_name)
            .ok_or_else(|| anyhow!("cannot find {}", machine_name))
    }

    pub fn list_non_proxies(&self) -> impl Iterator<Item = (&String, &ServerDef)> {
        self.0.iter().filter(|(_, v)| !v.proxy.unwrap_or(false))
    }
}

impl EnvironmentDef<ResourceDef> {
    pub fn get_resource(&self, resource_name: &str) -> Result<&ResourceDef> {
        self.0
            .get(resource_name)
            .ok_or(anyhow!("cannot find resource {}", resource_name))
    }
}

pub struct Ssh {
    args: Vec<String>,
    dest: String,
}

fn ssh_login(
    user: Option<&str>,
    resources: &Resource,
    target_env: &str,
    machine_name: &str,
) -> Result<Ssh> {
    let envdef = resources.get_target_env(target_env)?;
    let machine_def = envdef.get_machine(machine_name)?;

    let mut args = Vec::new();

    // user known hosts files option
    let mut path = ssh_dir();
    path.push(format!("known_hosts_machlist_{}", target_env));
    let hostfile = path.as_path().display().to_string();

    let user_known_host_arg = format!("-oUserKnownHostsFile={}", hostfile);

    args.push(user_known_host_arg);

    // jump option
    let jump = match &machine_def.jump {
        None => None,
        Some(jump_machine) => Some(envdef.get_machine(jump_machine)?),
    };

    match jump {
        None => (),
        Some(def) => {
            let ip = def.ip.clone().expect("jump proxy to have an ip");
            let jump_str = user_host(user.as_deref(), &ip);
            args.push("-J".to_string());
            args.push(jump_str);
        }
    };

    let ssh_dest = if let Some(ip) = &machine_def.ip {
        user_host(user.as_deref(), ip)
    } else if let Some(name) = &machine_def.name {
        user_host(user.as_deref(), name)
    } else {
        bail!("targetted machine doesn't have IP or name")
    };
    Ok(Ssh {
        args,
        dest: ssh_dest,
    })
}

fn shell(common: &CommonArgs, target_env: &str, machine_name: &str) -> Result<()> {
    let resources = parse_resources(&common.res_file)?;
    let user = resources.get_username()?;

    let ssh_opt = ssh_login(user.as_deref(), &resources, target_env, machine_name)?;

    println!(
        "connecting target environment={} dest={}",
        target_env, machine_name,
    );

    let mut command = Command::new("ssh");

    if common.verbose > 0 {
        command.arg("-v");
    }

    for a in ssh_opt.args.into_iter() {
        command.arg(a);
    }
    command.arg(ssh_opt.dest);
    command.exec();
    Ok(())
}

fn copy_from(
    common: &CommonArgs,
    target_env: &str,
    machine_name: &str,
    copy_path: &str,
) -> Result<()> {
    let resources = parse_resources(&common.res_file)?;
    let user = resources.get_username()?;

    let ssh_opt = ssh_login(user.as_deref(), &resources, target_env, machine_name)?;

    println!(
        "connecting target environment={} dest={}",
        target_env, machine_name
    );

    let mut command = Command::new("scp");

    if common.verbose > 0 {
        command.arg("-v");
    }

    for a in ssh_opt.args.into_iter() {
        command.arg(a);
    }
    let src = format!("{}:{}", ssh_opt.dest, copy_path);
    command.arg(src);
    command.arg("./");
    command.exec();
    Ok(())
}

fn copy_to(
    common: &CommonArgs,
    target_env: &str,
    machine_name: &str,
    copy_path: &str,
) -> Result<()> {
    let resources = parse_resources(&common.res_file)?;
    let user = resources.get_username()?;

    let ssh_opt = ssh_login(user.as_deref(), &resources, target_env, machine_name)?;

    println!(
        "connecting target environment={} dest={}",
        target_env, machine_name,
    );

    let mut command = Command::new("scp");

    if common.verbose > 0 {
        command.arg("-v");
    }

    for a in ssh_opt.args.into_iter() {
        command.arg(a);
    }
    let dst = format!("{}:", ssh_opt.dest);
    command.arg(copy_path);
    command.arg(dst);
    command.exec();
    Ok(())
}

fn tunnel(
    common: &CommonArgs,
    target_env: &str,
    resource_name: &str,
    local_port: Option<&str>,
) -> Result<()> {
    use std::str::FromStr;
    let local_port = local_port.map(|x| u16::from_str(x).expect("local port is not valid port"));

    let resources = parse_resources(&common.res_file)?;
    let user = resources.get_username()?;

    let defs = resources.get_target_env_resources(target_env)?;
    let def = defs.get_resource(resource_name)?;

    let machine_name = &def.server;
    let local_port = local_port.unwrap_or(def.port);

    let ssh_opt = ssh_login(user.as_deref(), &resources, target_env, machine_name)?;

    println!(
        "tunneling to target environment={} resource={} at port {}",
        resource_name, machine_name, local_port
    );

    let mut command = Command::new("ssh");

    if common.verbose > 0 {
        command.arg("-v");
    }

    for a in ssh_opt.args.into_iter() {
        command.arg(a);
    }

    command.arg("-N"); // do not execute a remote command
    command.arg("-L");

    let arg_forwarding = format!("{}:{}:{}", local_port, def.at, def.port);
    command.arg(arg_forwarding);

    command.arg(ssh_opt.dest);
    command.exec();
    Ok(())
}

fn list(common: &CommonArgs, target_env: &Option<&str>) -> Result<()> {
    let resources = parse_resources(&common.res_file)?;

    if let Some(target_env) = target_env {
        let envdef = resources.get_target_env(*target_env)?;
        for k in envdef.list_non_proxies().map(|(k, _)| k) {
            println!("{}", k)
        }
    } else {
        println!("listing all target environments");
        for k in resources.server.keys() {
            println!("{}", k)
        }
    }
    Ok(())
}

struct CommonArgs {
    verbose: u64,
    res_file: PathBuf,
}

fn main() -> Result<()> {
    const ARG_VERBOSE: &str = "verbose";
    const ARG_RES_FILE: &str = "res-file";

    const SUBCMD_SHELL: &str = "shell";
    const ARG_TARGET_ENV: &str = "target-env";
    const ARG_MACHINE: &str = "machine";

    const SUBCMD_LIST: &str = "list";

    const SUBCMD_COPY_FROM: &str = "copy-from";
    const ARG_COPY_FROM_PATH: &str = "copy-from-path";

    const SUBCMD_COPY_TO: &str = "copy-to";
    const ARG_COPY_TO_PATH: &str = "copy-to-path";

    const SUBCMD_TUNNEL: &str = "tunnel";
    const ARG_TUNNEL_RESOURCE: &str = "tunnel-resource";
    const ARG_TUNNEL_LOCAL_PORT: &str = "tunnel-local-port";

    let default_machlist_file = machlist_local().display().to_string();

    let arg_target_env = Arg::with_name(ARG_TARGET_ENV)
        .help("Target environment (alpha, prod, ..)")
        .takes_value(true)
        .short("t")
        .long("target");
    let arg_machine = Arg::with_name(ARG_MACHINE)
        .help("machine destination")
        .required(true);

    let app = App::new("machlist")
        .arg(
            Arg::with_name(ARG_VERBOSE)
                .global(true)
                .help("Increase client verbosity (use multiple times to increase)")
                .multiple(true)
                .short("v"),
        )
        .arg(
            Arg::with_name(ARG_RES_FILE)
                .help("TOML Resource file to use")
                .default_value(default_machlist_file.as_str())
                .global(true)
                .multiple(false)
                .takes_value(true)
                .short("r"),
        )
        .subcommand(
            SubCommand::with_name(SUBCMD_SHELL)
                .about("Shell on a given resource")
                .arg(&arg_target_env)
                .arg(&arg_machine),
        )
        .subcommand(
            SubCommand::with_name(SUBCMD_COPY_FROM)
                .about("Copy file from a given resource")
                .arg(&arg_target_env)
                .arg(&arg_machine)
                .arg(
                    Arg::with_name(ARG_COPY_FROM_PATH)
                        .help("Path to copy")
                        .required(true),
                ),
        )
        .subcommand(
            SubCommand::with_name(SUBCMD_COPY_TO)
                .about("Copy file to a given resource")
                .arg(&arg_target_env)
                .arg(&arg_machine)
                .arg(
                    Arg::with_name(ARG_COPY_TO_PATH)
                        .help("Path to copy")
                        .required(true),
                ),
        )
        .subcommand(
            SubCommand::with_name(SUBCMD_TUNNEL)
                .about("Make a tunnel to resource")
                .arg(&arg_target_env)
                .arg(
                    Arg::with_name(ARG_TUNNEL_RESOURCE)
                        .help("Resource on machine to open")
                        .required(true),
                )
                .arg(
                    Arg::with_name(ARG_TUNNEL_LOCAL_PORT)
                        .help("port to bind (default to resource define)")
                        .required(false),
                ),
        )
        .subcommand(
            SubCommand::with_name(SUBCMD_LIST)
                .about("List resources")
                .arg(arg_target_env),
        );
    let m = app.get_matches();

    let verbose = m.occurrences_of(ARG_VERBOSE);
    let res_file = m.value_of(ARG_RES_FILE).unwrap().into();

    let common = CommonArgs { verbose, res_file };

    const DEFAULT_ENV: &str = "alpha";

    if let Some(m) = m.subcommand_matches(SUBCMD_SHELL) {
        let target_env = m.value_of(ARG_TARGET_ENV).unwrap_or(DEFAULT_ENV);
        let machine = m.value_of(ARG_MACHINE).unwrap();
        shell(&common, &target_env, &machine)
    } else if let Some(m) = m.subcommand_matches(SUBCMD_LIST) {
        let target_env = m.value_of(ARG_TARGET_ENV);
        list(&common, &target_env)
    } else if let Some(m) = m.subcommand_matches(SUBCMD_COPY_FROM) {
        let target_env = m.value_of(ARG_TARGET_ENV).unwrap_or(DEFAULT_ENV);
        let machine = m.value_of(ARG_MACHINE).unwrap();
        let copy_path = m.value_of(ARG_COPY_FROM_PATH).unwrap();
        copy_from(&common, &target_env, machine, copy_path)
    } else if let Some(m) = m.subcommand_matches(SUBCMD_COPY_TO) {
        let target_env = m.value_of(ARG_TARGET_ENV).unwrap_or(DEFAULT_ENV);
        let machine = m.value_of(ARG_MACHINE).unwrap();
        let copy_path = m.value_of(ARG_COPY_TO_PATH).unwrap();
        copy_to(&common, &target_env, machine, copy_path)
    } else if let Some(m) = m.subcommand_matches(SUBCMD_TUNNEL) {
        let target_env = m.value_of(ARG_TARGET_ENV).unwrap_or(DEFAULT_ENV);
        let resource = m.value_of(ARG_TUNNEL_RESOURCE).unwrap();
        let local_port = m.value_of(ARG_TUNNEL_LOCAL_PORT);
        tunnel(&common, &target_env, resource, local_port)
    } else if let Some(name) = m.subcommand_name() {
        bail!("Unknown command {}", name);
    } else {
        bail!("No command specified");
    }
}
