use cargo_metadata::{CargoOpt, MetadataCommand};
use clap::Parser;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

/// CLI args for the `wasmer init` command
#[derive(Debug, Parser)]
pub struct Init {
    /// Initialize wapm.toml for a library package
    #[clap(long, group = "crate-type")]
    pub lib: bool,
    /// Initialize wapm.toml for a binary package
    #[clap(long, group = "crate-type")]
    pub bin: bool,
    /// Initialize an empty wapm.toml
    #[clap(long, group = "crate-type")]
    pub empty: bool,
    /// Force overwriting the wapm.toml, even if it already exists
    #[clap(long)]
    pub overwrite: bool,
    /// Don't display debug output
    #[clap(long)]
    pub quiet: bool,
    /// Ignore the existence of cargo wapm / cargo wasmer
    #[clap(long)]
    pub no_cargo_wapm: bool,
    /// Namespace to init with, default = current logged in user or _
    #[clap(long)]
    pub namespace: Option<String>,
    /// Version of the initialized package
    #[clap(long)]
    pub version: Option<semver::Version>,
    /// If the `manifest-path` is a Cargo.toml, use that file to initialize the wapm.toml
    #[clap(long)]
    pub manifest_path: Option<PathBuf>,
    /// Add default dependencies for common packages (currently supported: `python`, `js`)
    #[clap(long)]
    pub template: Option<String>,
    /// Include file paths into the target container filesystem
    #[clap(long)]
    pub include: Vec<String>,
    /// Directory of the output file name. wasmer init will error if the target dir
    /// already contains a wasmer.toml. Also sets the package name.
    #[clap(name = "PACKAGE_PATH")]
    pub out: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Copy, Clone)]
enum BinOrLib {
    Bin,
    Lib,
    Empty,
}

// minimal version of the Cargo.toml [package] section
#[derive(Debug, Clone)]
struct MiniCargoTomlPackage {
    name: String,
    version: semver::Version,
    description: Option<String>,
    homepage: Option<String>,
    repository: Option<String>,
    license: Option<String>,
    readme: Option<PathBuf>,
    license_file: Option<PathBuf>,
    #[allow(dead_code)]
    workspace_root: PathBuf,
    #[allow(dead_code)]
    build_dir: PathBuf,
}

static WASMER_TOML_NAME: &str = "wasmer.toml";

impl Init {
    /// `wasmer init` execution
    pub fn execute(&self) -> Result<(), anyhow::Error> {
        let bin_or_lib = self.get_bin_or_lib()?;

        let package_name;
        let target_file = match self.out.as_ref() {
            None => {
                let current_dir = std::env::current_dir()?;
                package_name = current_dir
                    .canonicalize()?
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow::anyhow!("no current dir name"))?;
                current_dir.join(WASMER_TOML_NAME)
            }
            Some(s) => {
                let _ = std::fs::create_dir_all(s);
                package_name = s
                    .canonicalize()?
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow::anyhow!("no dir name"))?;
                s.join(WASMER_TOML_NAME)
            }
        };

        if target_file.exists() && !self.overwrite {
            anyhow::bail!(
                "wapm project already initialized in {}",
                target_file.display(),
            );
        }

        // See if the directory has a Cargo.toml file, if yes, copy the license / readme, etc.
        let manifest_path = match self.manifest_path.as_ref() {
            Some(s) => s.clone(),
            None => {
                let cargo_toml_path = self
                    .out
                    .clone()
                    .unwrap_or_else(|| std::env::current_dir().unwrap())
                    .join("Cargo.toml");
                cargo_toml_path
                    .canonicalize()
                    .unwrap_or_else(|_| cargo_toml_path.clone())
            }
        };

        let cargo_toml = if manifest_path.exists() {
            use anyhow::Context;

            let mut metadata = MetadataCommand::new();
            metadata.manifest_path(&manifest_path);
            metadata.no_deps();
            metadata.features(CargoOpt::AllFeatures);

            let metadata = metadata.exec();

            let metadata = match metadata {
                Ok(o) => o,
                Err(e) => {
                    return Err(anyhow::anyhow!("failed to load metadata: {e}")
                        .context(anyhow::anyhow!("{}", manifest_path.display())));
                }
            };

            let package = metadata
                .root_package()
                .ok_or_else(|| anyhow::anyhow!("no root package found in cargo metadata"))
                .context(anyhow::anyhow!("{}", manifest_path.display()))?;

            Some(MiniCargoTomlPackage {
                name: package.name.clone(),
                version: package.version.clone(),
                description: package.description.clone(),
                homepage: package.homepage.clone(),
                repository: package.repository.clone(),
                license: package.license.clone(),
                readme: package.readme.clone().map(|s| s.into_std_path_buf()),
                license_file: package.license_file.clone().map(|f| f.into_std_path_buf()),
                workspace_root: metadata.workspace_root.into_std_path_buf(),
                build_dir: metadata
                    .target_directory
                    .into_std_path_buf()
                    .join("wasm32-wasi"),
            })
        } else {
            None
        };

        let package_name = cargo_toml
            .as_ref()
            .map(|p| &p.name)
            .unwrap_or(&package_name);

        let namespace = self.namespace.clone().unwrap_or_else(|| {
            let username = wasmer_registry::whoami(None, None).ok().map(|o| o.1);
            username
                .or_else(|| package_name.split('/').next().map(|s| s.to_string()))
                .unwrap_or_else(|| "_".to_string())
        });
        let module_name = package_name
            .split('/')
            .last()
            .unwrap_or(package_name)
            .to_string();
        let version = self.version.clone().unwrap_or_else(|| {
            cargo_toml
                .as_ref()
                .map(|t| t.version.clone())
                .unwrap_or_else(|| semver::Version::parse("0.1.0").unwrap())
        });
        let license = cargo_toml.as_ref().and_then(|t| t.license.clone());
        let license_file = cargo_toml.as_ref().and_then(|t| t.license_file.clone());
        let readme = cargo_toml.as_ref().and_then(|t| t.readme.clone());
        let repository = cargo_toml.as_ref().and_then(|t| t.repository.clone());
        let homepage = cargo_toml.as_ref().and_then(|t| t.homepage.clone());
        let description = cargo_toml
            .as_ref()
            .and_then(|t| t.description.clone())
            .unwrap_or_else(|| format!("Description for package {module_name}"));

        let default_abi = wapm_toml::Abi::Wasi;
        let bindings = Self::get_bindings(&target_file, bin_or_lib);
        let modules = vec![wapm_toml::Module {
            name: module_name.to_string(),
            source: cargo_toml
                .as_ref()
                .map(|p| {
                    // Normalize the path to /target/release to be relative to the parent of the Cargo.toml
                    let outpath = p
                        .build_dir
                        .join("release")
                        .join(&format!("{module_name}.wasm"));
                    let canonicalized_outpath = outpath.canonicalize().unwrap_or(outpath);
                    let outpath_str = format!("{}", canonicalized_outpath.display());
                    let manifest_canonicalized = manifest_path
                        .parent()
                        .and_then(|p| p.canonicalize().ok())
                        .unwrap_or_else(|| manifest_path.clone());
                    let manifest_str = format!("{}/", manifest_canonicalized.display());
                    let relative_str = outpath_str.replacen(&manifest_str, "", 1);
                    Path::new(&relative_str).to_path_buf()
                })
                .unwrap_or_else(|| Path::new(&format!("{module_name}.wasm")).to_path_buf()),
            kind: None,
            abi: default_abi,
            bindings: bindings.clone(),
            interfaces: Some({
                let mut map = HashMap::new();
                map.insert("wasi".to_string(), "0.1.0-unstable".to_string());
                map
            }),
        }];

        let constructed_manifest = wapm_toml::Manifest {
            package: wapm_toml::Package {
                name: format!("{namespace}/{module_name}"),
                version,
                description,
                license,
                license_file,
                readme,
                repository,
                homepage,
                wasmer_extra_flags: None,
                disable_command_rename: false,
                rename_commands_to_raw_command_name: false,
            },
            dependencies: self.get_dependencies(),
            command: Self::get_command(&modules, bin_or_lib),
            module: match bin_or_lib {
                BinOrLib::Empty => None,
                _ => Some(modules),
            },
            fs: self.get_filesystem_mapping(),
            base_directory_path: target_file
                .parent()
                .map(|o| o.to_path_buf())
                .unwrap_or_else(|| target_file.clone()),
        };

        if let Some(parent) = target_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // Test if cargo wapm is installed
        let cargo_wapm_present = if self.no_cargo_wapm {
            false
        } else {
            let cargo_wapm_stdout = std::process::Command::new("cargo")
                .arg("wapm")
                .arg("--version")
                .output()
                .map(|s| String::from_utf8_lossy(&s.stdout).to_string())
                .unwrap_or_default();

            cargo_wapm_stdout.lines().count() == 1
                && (cargo_wapm_stdout.contains("cargo wapm")
                    || cargo_wapm_stdout.contains("cargo-wapm"))
        };

        // if Cargo.toml is present and cargo wapm is installed, add the
        // generated manifest to the Cargo.toml instead of creating a new wapm.toml
        let should_add_to_cargo_toml = cargo_toml.is_some() && cargo_wapm_present;

        // If the Cargo.toml is present, but cargo wapm is not installed,
        // generate a wapm.toml, but notify the user about installing cargo-wapm
        if !cargo_wapm_present && !self.no_cargo_wapm && cargo_toml.is_some() && !self.quiet {
            eprintln!(
                "Note: you seem to have a Cargo.toml file, but you haven't installed `cargo wapm`."
            );
            eprintln!("You can build and release Rust projects directly with `cargo wapm publish`: https://crates.io/crates/cargo-wapm");
            eprintln!("Install it with:");
            eprintln!();
            eprintln!("    cargo install cargo-wapm");
            eprintln!();
        }

        let note =
            "# See more keys and definitions at https://docs.wasmer.io/ecosystem/wapm/manifest";

        // generate the wasmer.toml and exit
        if !should_add_to_cargo_toml {
            let toml_string = toml::to_string_pretty(&constructed_manifest)?
                .replace("[dependencies]", &format!("{note}\r\n\r\n[dependencies]"))
                .lines()
                .collect::<Vec<_>>()
                .join("\r\n");

            std::fs::write(&target_file, &toml_string)
                .with_context(|| format!("Unable to write to \"{}\"", target_file.display()))?;

            return Ok(());
        }

        // add the manifest to the Cargo.toml
        let old_cargo = std::fs::read_to_string(&manifest_path).unwrap();

        // if the Cargo.toml already contains a [metadata.wapm] section, don't generate it again
        if old_cargo.contains("metadata.wapm") && !self.overwrite {
            return Err(anyhow::anyhow!(
                "wapm project already initialized in Cargo.toml file"
            ));
        }

        // generate the Wapm struct for the [metadata.wapm] table
        // and add it to the end of the file
        let metadata_wapm = wapm_toml::rust::Wapm {
            namespace,
            package: Some(module_name),
            wasmer_extra_flags: None,
            abi: default_abi,
            fs: constructed_manifest.fs,
            bindings,
        };

        let toml_string = toml::to_string_pretty(&metadata_wapm)?
            .replace("[dependencies]", &format!("{note}\r\n\r\n[dependencies]"))
            .lines()
            .collect::<Vec<_>>()
            .join("\r\n");

        if !self.quiet {
            eprintln!(
                "You have cargo-wapm installed, added metadata to Cargo.toml instead of wasmer.toml"
            );
            eprintln!("Build and publish your package with:");
            eprintln!();
            eprintln!("    cargo wapm");
            eprintln!();
        }

        std::fs::write(
            &manifest_path,
            &format!("{old_cargo}\r\n\r\n[package.metadata.wapm]\r\n{toml_string}"),
        )?;

        Ok(())
    }

    fn get_filesystem_mapping(&self) -> Option<HashMap<String, PathBuf>> {
        if self.include.is_empty() {
            return None;
        }

        let include = self
            .include
            .iter()
            .map(|path| {
                if path == "." || path == "/" {
                    return ("/".to_string(), Path::new("/").to_path_buf());
                }

                let key = format!("./{path}");
                let value = Path::new(&format!("/{path}")).to_path_buf();

                (key, value)
            })
            .collect();

        Some(include)
    }

    fn get_command(
        modules: &[wapm_toml::Module],
        bin_or_lib: BinOrLib,
    ) -> Option<Vec<wapm_toml::Command>> {
        match bin_or_lib {
            BinOrLib::Bin => Some(
                modules
                    .iter()
                    .map(|m| {
                        wapm_toml::Command::V1(wapm_toml::CommandV1 {
                            name: m.name.clone(),
                            module: m.name.clone(),
                            main_args: None,
                            package: None,
                        })
                    })
                    .collect(),
            ),
            BinOrLib::Lib | BinOrLib::Empty => None,
        }
    }

    /// Returns the dependencies based on the `--template` flag
    fn get_dependencies(&self) -> Option<HashMap<String, String>> {
        Some({
            match self.template.as_deref() {
                Some("js") => {
                    let mut map = HashMap::default();
                    map.insert("python".to_string(), "quickjs/quickjs@latest".to_string());
                    map
                }
                Some("python") => {
                    let mut map = HashMap::default();
                    map.insert("python".to_string(), "python/python@latest".to_string());
                    map
                }
                _ => HashMap::default(),
            }
        })
    }

    // Returns whether the template for the wapm.toml should be a binary, a library or an empty file
    fn get_bin_or_lib(&self) -> Result<BinOrLib, anyhow::Error> {
        match (self.empty, self.bin, self.lib) {
            (true, true, _) | (true, _, true) => Err(anyhow::anyhow!(
                "cannot combine --empty with --bin or --lib"
            )),
            (true, false, false) => Ok(BinOrLib::Empty),
            (_, true, true) => Err(anyhow::anyhow!(
                "cannot initialize a wapm manifest with both --bin and --lib, pick one"
            )),
            (false, true, _) => Ok(BinOrLib::Bin),
            (false, _, true) => Ok(BinOrLib::Lib),
            _ => Ok(BinOrLib::Bin),
        }
    }

    fn get_bindings(target_file: &Path, bin_or_lib: BinOrLib) -> Option<wapm_toml::Bindings> {
        match bin_or_lib {
            BinOrLib::Bin | BinOrLib::Empty => None,
            BinOrLib::Lib => target_file.parent().and_then(|parent| {
                walkdir::WalkDir::new(parent)
                    .min_depth(1)
                    .max_depth(3)
                    .follow_links(false)
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let is_wit = e.path().extension().and_then(|s| s.to_str()) == Some(".wit");
                        let is_wai = e.path().extension().and_then(|s| s.to_str()) == Some(".wai");
                        if is_wit {
                            Some(wapm_toml::Bindings::Wit(wapm_toml::WitBindings {
                                wit_exports: e.path().to_path_buf(),
                                wit_bindgen: semver::Version::parse("0.1.0").unwrap(),
                            }))
                        } else if is_wai {
                            Some(wapm_toml::Bindings::Wai(wapm_toml::WaiBindings {
                                exports: None,
                                imports: vec![e.path().to_path_buf()],
                                wai_version: semver::Version::parse("0.1.0").unwrap(),
                            }))
                        } else {
                            None
                        }
                    })
                    .next()
            }),
        }
    }
}
