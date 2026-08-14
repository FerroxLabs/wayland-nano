use nano_agent::mcp::McpServerSpec;
use nano_plugins::{
    PluginError, PluginStore, RegistrySource, fetch, manifest::MarketplaceManifest,
};
use std::{
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
};

pub fn plugin_mcp_specs(nano_home: &Path) -> Result<Vec<McpServerSpec>, PluginError> {
    nano_plugins::store::plugin_mcp_specs(nano_home)
}
pub fn plugin_skill_roots(nano_home: &Path) -> Result<Vec<PathBuf>, PluginError> {
    nano_plugins::store::plugin_skill_roots(nano_home)
}

pub fn run(home: &Path, args: &[String], out: &mut dyn Write) -> i32 {
    match run_inner(home, args, out) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("wayland-nano: {e}");
            2
        }
    }
}
fn run_inner(home: &Path, args: &[String], out: &mut dyn Write) -> Result<(), PluginError> {
    let store = PluginStore::new(home);
    match args.first().map(String::as_str) {
        Some("registry") => registry(&store, &args[1..], out),
        Some("install") => {
            let target = args.get(1).ok_or_else(usage)?;
            let yes = args[2..].iter().any(|a| a == "--yes");
            if args[2..].iter().any(|a| a != "--yes") {
                return Err(usage());
            }
            install(&store, target, yes, out)
        }
        Some("list") if args.len() == 1 => {
            for p in store.installed()? {
                writeln!(
                    out,
                    "{}/{}\t{:?}\t{}\t{}\t{}\t{}",
                    p.registry,
                    p.plugin,
                    p.kind,
                    p.source,
                    &p.archive_sha256[..16],
                    p.resolved_ref,
                    p.installed_at
                )
                .map_err(ioerr)?;
            }
            Ok(())
        }
        Some("remove") if args.len() == 2 => {
            let (plugin, reg) = split_required(&args[1])?;
            store.remove_plugin(plugin, reg)
        }
        _ => Err(usage()),
    }
}
fn registry(store: &PluginStore, args: &[String], out: &mut dyn Write) -> Result<(), PluginError> {
    match args.first().map(String::as_str) {
        Some("add") if args.len() >= 4 => {
            let name = &args[1];
            let source = match args[2].as_str() {
                "local-dir" if args.len() == 4 => RegistrySource::LocalDir {
                    path: PathBuf::from(&args[3]),
                },
                "github" if args.len() == 4 => RegistrySource::github(&args[3])?,
                _ => return Err(usage()),
            };
            store.add_registry(name, source)
        }
        Some("list") if args.len() == 1 => {
            for r in store.registries()? {
                writeln!(out, "{}\t{}\t{}", r.name, r.reg_key, r.source.display())
                    .map_err(ioerr)?;
            }
            Ok(())
        }
        Some("remove") if args.len() == 2 => store.remove_registry(&args[1]),
        _ => Err(usage()),
    }
}
fn install(
    store: &PluginStore,
    target: &str,
    yes: bool,
    out: &mut dyn Write,
) -> Result<(), PluginError> {
    let (name, qual) = split_optional(target);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| PluginError::Invalid(format!("runtime unavailable: {e}")))?;
    let mut matches = Vec::new();
    for reg in store.registries()? {
        if qual.is_some_and(|q| q != reg.name) {
            continue;
        }
        let cache = store.root().join("registries").join(&reg.reg_key);
        let fetched = rt.block_on(fetch::fetch(&reg.source, &cache))?;
        let market = MarketplaceManifest::load(&fetched.root.join("marketplace.json"))?;
        if market.plugins.iter().any(|p| p.name == name) {
            matches.push((reg, fetched));
        }
    }
    if matches.is_empty() {
        return Err(PluginError::NotFound(target.into()));
    }
    if matches.len() > 1 {
        return Err(PluginError::Ambiguous(
            matches
                .iter()
                .map(|(r, _)| format!("{name}@{}", r.name))
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    let (reg, fetched) = matches.pop().unwrap();
    let plan = store.preview_install(
        &reg,
        &fetched.root,
        &fetched.resolved_ref,
        &fetched.archive_sha256,
        name,
    )?;
    write!(out, "{}", plan.render()).map_err(ioerr)?;
    out.flush().map_err(ioerr)?;
    if !yes {
        if !std::io::stdin().is_terminal() {
            return Err(PluginError::ConsentRequired);
        }
        eprint!("Install this unsigned plugin? [y/N] ");
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| PluginError::io("stdin", e))?;
        if !matches!(line.trim(), "y" | "Y" | "yes" | "YES") {
            return Err(PluginError::ConsentRequired);
        }
    }
    store.install(
        &reg,
        &fetched.root,
        &fetched.resolved_ref,
        &fetched.archive_sha256,
        name,
    )?;
    writeln!(out, "installed {}/{}", reg.name, name).map_err(ioerr)?;
    Ok(())
}
fn split_optional(value: &str) -> (&str, Option<&str>) {
    value
        .rsplit_once('@')
        .map_or((value, None), |(p, r)| (p, Some(r)))
}
fn split_required(value: &str) -> Result<(&str, &str), PluginError> {
    let (p, r) = value
        .rsplit_once('@')
        .ok_or_else(|| PluginError::Invalid("remove requires plugin@registry".into()))?;
    if p.is_empty() || r.is_empty() {
        return Err(usage());
    }
    Ok((p, r))
}
fn usage() -> PluginError {
    PluginError::Invalid("usage: wayland-nano plugin registry add|list|remove ... | install <plugin>[@registry] [--yes] | list | remove <plugin>@<registry>".into())
}
fn ioerr(e: std::io::Error) -> PluginError {
    PluginError::io("stdout", e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn fixture(root: &Path) {
        fs::create_dir_all(root.join("p/skills/hello")).unwrap();
        fs::write(root.join("marketplace.json"),r#"{"name":"r","version":1,"plugins":[{"name":"p","version":null,"description":null,"source":{"kind":"path","path":"p"}}]}"#).unwrap();
        fs::write(
            root.join("p/plugin.json"),
            r#"{"name":"p","version":null,"description":null,"kind":"skills","mcp_server":null}"#,
        )
        .unwrap();
        fs::write(root.join("p/skills/hello/SKILL.md"), "# Hello").unwrap();
    }
    #[test]
    fn local_install_list_remove() {
        let d = tempfile::tempdir().unwrap();
        let reg = d.path().join("reg");
        fixture(&reg);
        let home = d.path().join("home");
        let mut out = Vec::new();
        let a = vec![
            "registry".into(),
            "add".into(),
            "r".into(),
            "local-dir".into(),
            reg.display().to_string(),
        ];
        assert_eq!(run(&home, &a, &mut out), 0);
        let i = vec!["install".into(), "p@r".into(), "--yes".into()];
        assert_eq!(run(&home, &i, &mut out), 0);
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(text.contains("UNSIGNED") && text.contains("sha256"));
        assert_eq!(plugin_skill_roots(&home).unwrap().len(), 1);
        let rm = vec!["remove".into(), "p@r".into()];
        assert_eq!(run(&home, &rm, &mut out), 0);
        assert!(plugin_skill_roots(&home).unwrap().is_empty());
        assert!(home.join("wayland-nano-plugins/journal.jsonl").is_file());
    }
}
