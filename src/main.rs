use anyhow::{anyhow, bail, Result};
use clap::{App, Arg, SubCommand};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, Deserialize)]
struct Resource {
    username: Option<String>,
    server: HashMap<String, EnvironmentDef>,
}

#[derive(Clone, Debug, Deserialize)]
struct EnvironmentDef(HashMap<String, ServerDef>);

#[derive(Clone, Debug, Deserialize)]
struct ServerDef {
    ip: Option<String>,
    name: Option<String>,
    jump: Option<String>,
    proxy: Option<bool>,
}

fn user_host(user: Option<&str>, host: &str) -> String {
    match user {
        Some(u) => format!("{}@{}", u, host),
        None => format!("{}", host),
    }
}

fn parse_resources<P: AsRef<Path>>(file: Option<P>) -> Result<Resource> {
    let content = match file {
        None => std::fs::read_to_string("resources.toml")?,
        Some(p) => std::fs::read_to_string(p)?,
    };

    let values: Resource = toml::de::from_str(&content)?;
    Ok(values)
}

impl Resource {
    pub fn get_target_env(&self, target_env: &str) -> Result<&EnvironmentDef> {
        self.server
            .get(target_env)
            .ok_or(anyhow!("cannot find specified target environment"))
    }

    pub fn get_username(&self) -> Result<Option<String>> {
        match &self.username {
            None => Ok(None),
            Some(u) => {
                if let Some(env_name) = u.strip_prefix("env:") {
                    Ok(Some(std::env::var(env_name)?))
                } else {
                    Ok(Some(u.clone()))
                }
            }
        }
    }
}

impl EnvironmentDef {
    pub fn get_machine(&self, machine_name: &str) -> Result<&ServerDef> {
        self.0
            .get(machine_name)
            .ok_or(anyhow!("cannot find {}", machine_name))
    }

    pub fn list_non_proxies(&self) -> impl Iterator<Item = (&String, &ServerDef)> {
        self.0.iter().filter(|(_, v)| !v.proxy.unwrap_or(false))
    }
}

fn ssh(common: &CommonArgs, target_env: &str, machine_name: &str) -> Result<()> {
    let resources = parse_resources(common.res_file.as_ref())?;

    let envdef = resources.get_target_env(target_env)?;
    let machine_def = envdef.get_machine(machine_name)?;

    let jump = match &machine_def.jump {
        None => None,
        Some(jump_machine) => Some(envdef.get_machine(jump_machine)?),
    };

    let mut command = Command::new("ssh");

    if common.verbose > 0 {
        command.arg("-v");
    }

    let hostfile = format!("{}/.ssh/known_hosts_machlist_{}", env!("HOME"), target_env);

    let user_known_host_arg = format!("-oUserKnownHostsFile={}", hostfile);

    command.arg(user_known_host_arg);

    let user = resources.get_username()?;

    match jump {
        None => (),
        Some(def) => {
            let ip = def.ip.clone().expect("jump proxy to have an ip");
            let jump_str = user_host(user.as_deref(), &ip);
            command.arg("-J").arg(jump_str);
        }
    };

    let ssh_dest = if let Some(ip) = &machine_def.ip {
        user_host(user.as_deref(), ip)
    } else if let Some(name) = &machine_def.name {
        user_host(user.as_deref(), name)
    } else {
        bail!("targetted machine doesn't have IP or name")
    };

    println!(
        "connecting target environment={} dest={}: {:?}",
        machine_name, target_env, ssh_dest
    );

    command.arg(ssh_dest);

    use std::os::unix::process::CommandExt;

    command.exec();
    Ok(())
}

fn list(common: &CommonArgs, target_env: &Option<&str>) -> Result<()> {
    let resources = parse_resources(common.res_file.as_ref())?;

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
    res_file: Option<PathBuf>,
}

fn main() -> Result<()> {
    const ARG_VERBOSE: &str = "verbose";
    const ARG_RES_FILE: &str = "res-file";

    const SUBCMD_SSH: &str = "ssh";
    const ARG_TARGET_ENV: &str = "target-env";
    const ARG_MACHINE: &str = "machine";

    const SUBCMD_LIST: &str = "list";

    let arg_target_env = Arg::with_name(ARG_TARGET_ENV)
        .help("Target environment (alpha, prod, ..)")
        .takes_value(true)
        .short("t")
        .long("target");

    let app = App::new("machlist")
        .arg(
            Arg::with_name(ARG_VERBOSE)
                .help("Increase client verbosity (use multiple times to increase)")
                .multiple(true)
                .short("v"),
        )
        .arg(
            Arg::with_name(ARG_RES_FILE)
                .help("TOML Resource file to use (default: resources.toml)")
                .multiple(false)
                .takes_value(true)
                .short("r"),
        )
        .subcommand(
            SubCommand::with_name(SUBCMD_SSH)
                .about("Ssh to a given resource")
                .arg(&arg_target_env)
                .arg(
                    Arg::with_name(ARG_MACHINE)
                        .help("machine destination")
                        .required(true),
                ),
        )
        .subcommand(
            SubCommand::with_name(SUBCMD_LIST)
                .about("List resources")
                .arg(arg_target_env),
        );
    let m = app.get_matches();

    let verbose = m.occurrences_of(ARG_VERBOSE);
    let res_file = m.value_of(ARG_RES_FILE); // .unwrap_or("resources.toml");

    let common = CommonArgs {
        verbose,
        res_file: res_file.map(|v| v.to_string().into()),
    };

    if let Some(m) = m.subcommand_matches(SUBCMD_SSH) {
        let target_env = m.value_of(ARG_TARGET_ENV).unwrap_or("alpha");
        let machine = m.value_of(ARG_MACHINE).unwrap();
        ssh(&common, &target_env, &machine)
    } else if let Some(m) = m.subcommand_matches(SUBCMD_LIST) {
        let target_env = m.value_of(ARG_TARGET_ENV);
        list(&common, &target_env)
    } else if let Some(name) = m.subcommand_name() {
        bail!("Unknown command {}", name);
    } else {
        bail!("No command specified");
    }
}
