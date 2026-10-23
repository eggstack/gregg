//! The GUI's host management.
//!
//! # What this module is
//!
//! The panel can add, remove and edit the hosts in the Gregg configuration
//! file. This module is the **seam** between the panel and that file: it
//! describes what the operator asked for, and performs it through the
//! **CLI's own library code** rather than through a second implementation of
//! the rules.
//!
//! # Why the CLI's code and not a second copy of it
//!
//! The endpoint grammar is not a detail: `gregg add` refuses a portless
//! host, refuses a nickname without a host, refuses HTTPS, and refuses an
//! inline nickname combined with `--name`. The config store takes a lock,
//! validates the **whole** config, and writes it atomically. A GUI that
//! re-implemented any of that would be a second set of rules that could
//! disagree with the CLI's, and the disagreement would only show up as a
//! config the CLI refuses and the GUI wrote.
//!
//! So the mutation goes through [`gregg::cli::add_host`] and
//! [`gregg::cli::remove_host`], which are the code paths `gregg add` and
//! `gregg remove` take. The GUI inherits the grammar, the lock, the
//! validation and the atomic write unchanged.
//!
//! Those two functions are exposed on their own rather than reached through
//! `gregg::cli::dispatch`, and the reason is measured. `dispatch` also
//! reaches the daemon commands, and the daemon reaches the HTTP client and
//! its TLS stack: a GUI that called `dispatch` linked all of that into its
//! binary — measured at 14,761,600 bytes against 10,803,360 before the
//! change — for two commands that never touch it. Calling the narrow entry
//! points keeps the rules and drops the weight.
//!
//! # The one-writer statement
//!
//! The client daemon is the only writer of *fleet state* and the only
//! *poller*. The config **file** may be written by the CLI or by this GUI,
//! never concurrently: both go through the same lock inside
//! [`gregg::config::ConfigStore::mutate_with_result`]. Every write here is
//! followed by an explicit reload, so there is no watcher and no background
//! mutation.
//!
//! # The endpoint change is a replacement
//!
//! Changing a host's endpoint is **not** an edit. The endpoint is part of the
//! system's identity, so the stable id changes with it, and the host's
//! selection and history belong to the old id. The panel says so before it
//! applies one, and the operator confirms it explicitly. That is the same
//! reason the TUI treats a re-pointed endpoint as a different host.

use std::path::PathBuf;

use gregg::cli;
use gregg::config::ConfigStore;

/// What the operator asked the configuration to do.
///
/// This is a *description*, not an action: it says what the panel wants, and
/// [`HostMutations::apply`] decides how. Keeping the two apart is what lets
/// the panel's behaviour be tested without a desktop and without a real
/// configuration file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostMutation {
    /// Add a host.
    Add {
        /// The endpoint, in the CLI's grammar: `host:port`, `[ipv6]:port`,
        /// `http://host:port/`, or `nickname@host:port`.
        endpoint: String,
        /// An optional display name.
        name: Option<String>,
    },
    /// Remove a host.
    Remove {
        /// The endpoint to remove. The CLI's `remove` also accepts a host on
        /// its own, which removes every entry for that host.
        endpoint: String,
    },
    /// Change a host's name, against the same endpoint.
    ///
    /// This is an **in-place** change: the host keeps its identity, because
    /// the endpoint — the thing the identity is made of — did not move.
    Rename {
        /// The endpoint the host is at.
        endpoint: String,
        /// The new name, or `None` to clear the configured name.
        name: Option<String>,
    },
    /// Point a host at a different endpoint.
    ///
    /// This is a **replacement**: the endpoint is part of the host's
    /// identity, so the host's stable id changes and its history does not
    /// follow it.
    Replace {
        /// The endpoint the host is at now.
        from: String,
        /// The endpoint it is moving to.
        to: String,
        /// The name the new host carries.
        name: Option<String>,
    },
}

impl HostMutation {
    /// A short description of the mutation, for the panel's message line.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Add { endpoint, .. } => format!("add {endpoint}"),
            Self::Remove { endpoint } => format!("remove {endpoint}"),
            Self::Rename { endpoint, .. } => format!("rename {endpoint}"),
            Self::Replace { from, to, .. } => format!("replace {from} with {to}"),
        }
    }
}

/// The seam the panel's mutations go through.
///
/// The panel never touches a configuration file itself: it describes a
/// mutation and hands it here. A test injects a fake seam and never writes a
/// developer's configuration; the real implementation is
/// [`ConfigMutations`].
pub trait HostMutations {
    /// Apply one mutation.
    ///
    /// Returns a message for the operator on success, or the reason the
    /// mutation was refused. A refusal never leaves the configuration
    /// changed.
    ///
    /// # Errors
    ///
    /// Returns the reason the mutation could not be applied.
    fn apply(&self, mutation: &HostMutation) -> Result<String, String>;
}

/// The real seam: the configuration file, mutated through the CLI's code.
pub struct ConfigMutations {
    /// The store the daemon's own configuration is read from.
    store: ConfigStore,
}

impl ConfigMutations {
    /// Build the seam for one configuration file.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            store: ConfigStore::new(path),
        }
    }

    /// The store this seam writes through.
    #[cfg(test)]
    #[must_use]
    pub fn store(&self) -> &ConfigStore {
        &self.store
    }

    /// Run one CLI operation and turn its outcome into a message.
    fn run(&self, outcome: Result<(), Box<dyn std::error::Error>>) -> Result<String, String> {
        match outcome {
            Ok(()) => Ok(String::new()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// How many hosts the configuration holds right now.
    fn host_count(&self) -> usize {
        self.store
            .load_or_default()
            .map(|config| config.systems.len())
            .unwrap_or(0)
    }

    /// Whether the configuration still holds a host at this endpoint.
    fn has_endpoint(&self, endpoint: &str) -> bool {
        let Ok(spec) = gregg::endpoint::EndpointSpec::parse_host_only(endpoint) else {
            return false;
        };
        self.store
            .load_or_default()
            .map(|config| {
                config.systems.iter().any(|system| {
                    system.port == spec.port && system.host.eq_ignore_ascii_case(&spec.host)
                })
            })
            .unwrap_or(false)
    }
}

impl HostMutations for ConfigMutations {
    fn apply(&self, mutation: &HostMutation) -> Result<String, String> {
        match mutation {
            HostMutation::Add { endpoint, name } => {
                // `replace` is deliberately **off**: an add that collides with
                // an existing host must be refused, not silently overwrite
                // that host's name.
                self.run(cli::add_host(&self.store, endpoint, name.as_deref(), false))?;
                Ok(format!("added {endpoint}"))
            }
            HostMutation::Remove { endpoint } => {
                let before = self.host_count();
                self.run(cli::remove_host(&self.store, endpoint))?;
                let after = self.host_count();
                if after == before {
                    // The CLI's `remove` succeeds when it matched nothing, and
                    // says so on stderr. The panel has to say it too, or the
                    // operator is told a host was removed when it was not.
                    return Err(format!("no configured host matches {endpoint}"));
                }
                Ok(format!("removed {endpoint}"))
            }
            HostMutation::Rename { endpoint, name } => {
                // The CLI's `--replace` keeps the existing entry's stable id
                // and its place in the configuration, and sets the new name.
                // That is exactly a rename: same endpoint, same identity.
                // An empty name clears the configured name, which is what the
                // CLI's own `add` does when it is given no name at all.
                self.run(cli::add_host(&self.store, endpoint, name.as_deref(), true))?;
                Ok(match name {
                    Some(name) => format!("renamed {endpoint} to {name}"),
                    None => format!("cleared the name of {endpoint}"),
                })
            }
            HostMutation::Replace { from, to, name } => {
                // The new endpoint is added **first**, so that a refusal —
                // a malformed endpoint, or one that is already configured —
                // happens before anything is removed and leaves the
                // configuration byte-identical.
                self.run(cli::add_host(&self.store, to, name.as_deref(), false))?;
                // The old endpoint is removed afterwards. The removal is
                // exact: the CLI's `remove` matches the host and the port.
                self.run(cli::remove_host(&self.store, from))?;
                if self.has_endpoint(from) {
                    // The replacement is not complete, and saying so is
                    // better than claiming a host moved when it did not.
                    return Err(format!(
                        "{from} is still configured: the replacement did not complete"
                    ));
                }
                Ok(format!("replaced {from} with {to}"))
            }
        }
    }
}

/// What the panel's own form state is.
///
/// The form is the panel's transient state: which host is being edited, what
/// the operator has typed, and what the last attempt said. It is kept apart
/// from the view model so that a failed mutation cannot disturb what the
/// panel is showing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostForm {
    /// Which host the form is about, if any.
    pub target: Option<HostTarget>,
    /// The endpoint the operator has typed.
    pub endpoint: String,
    /// The name the operator has typed.
    pub name: String,
}

/// The host a form is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostTarget {
    /// The host's stable id.
    pub id: String,
    /// The endpoint the host is at now.
    pub endpoint: String,
    /// The configured name, if the host has one.
    pub name: Option<String>,
}

impl HostForm {
    /// A form for adding a host.
    #[must_use]
    pub fn adding() -> Self {
        Self::default()
    }

    /// A form for editing one host.
    ///
    /// The endpoint field starts as the host's current endpoint, so the
    /// operator edits what is there rather than retyping it.
    #[must_use]
    pub fn editing(target: HostTarget) -> Self {
        Self {
            endpoint: target.endpoint.clone(),
            name: target.name.clone().unwrap_or_default(),
            target: Some(target),
        }
    }

    /// Whether the endpoint field differs from the endpoint the host is at.
    ///
    /// This is what makes an endpoint change a **replacement** rather than an
    /// edit: the field has moved off the host's own endpoint, so the host's
    /// identity is about to change.
    #[must_use]
    pub fn endpoint_changed(&self) -> bool {
        match &self.target {
            Some(target) => !self.endpoint.trim().eq_ignore_ascii_case(&target.endpoint),
            None => false,
        }
    }

    /// Whether the name field differs from the host's configured name.
    #[must_use]
    pub fn name_changed(&self) -> bool {
        let typed = self.name.trim();
        match &self.target {
            Some(target) => typed != target.name.as_deref().unwrap_or("").trim(),
            None => !typed.is_empty(),
        }
    }

    /// The name to send, or `None` to clear it.
    #[must_use]
    pub fn name_to_send(&self) -> Option<String> {
        let typed = self.name.trim();
        if typed.is_empty() {
            None
        } else {
            Some(typed.to_owned())
        }
    }

    /// The mutation the form describes, if it describes one.
    ///
    /// `None` means the form is in a state where nothing should be applied:
    /// an add with no endpoint, or an edit whose endpoint and name are both
    /// unchanged.
    #[must_use]
    pub fn to_mutation(&self) -> Option<HostMutation> {
        let endpoint = self.endpoint.trim();
        match &self.target {
            None => {
                if endpoint.is_empty() {
                    None
                } else {
                    Some(HostMutation::Add {
                        endpoint: endpoint.to_owned(),
                        name: self.name_to_send(),
                    })
                }
            }
            Some(target) => {
                if self.endpoint_changed() {
                    Some(HostMutation::Replace {
                        from: target.endpoint.clone(),
                        to: endpoint.to_owned(),
                        name: self.name_to_send(),
                    })
                } else if self.name_changed() {
                    Some(HostMutation::Rename {
                        endpoint: target.endpoint.clone(),
                        name: self.name_to_send(),
                    })
                } else {
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gregg::config::SystemEntry;

    /// A temporary directory that cleans itself up.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "gregg-gui-hosts-{label}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("creates the temporary directory");
            Self(path)
        }

        fn config(&self) -> PathBuf {
            self.0.join("gregg.toml")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The seam over one temporary configuration file.
    fn seam(dir: &TempDir) -> ConfigMutations {
        ConfigMutations::new(dir.config())
    }

    /// Write a configuration with one host.
    ///
    /// The store's whole configuration is written rather than mutated: the
    /// store's own error type carries a `toml::de::Error`, which is larger
    /// on Windows, and a closure over it trips a lint there that the root
    /// workspace has already allowed for the same reason.
    fn seed(dir: &TempDir, host: &str, port: u16, name: Option<&str>) {
        let store = ConfigStore::new(dir.config());
        let mut config = gregg::config::Config::default();
        config.systems.push(SystemEntry {
            id: "11111111-1111-1111-1111-111111111111".to_owned(),
            host: host.to_owned(),
            port,
            name: name.map(str::to_owned),
        });
        store.write(&config).expect("seeds the configuration");
    }

    /// The configuration's bytes, for the "byte-identical" assertions.
    fn bytes(dir: &TempDir) -> Vec<u8> {
        std::fs::read(dir.config()).expect("reads the configuration")
    }

    #[test]
    fn an_add_persists_and_is_visible_to_the_next_load() {
        let dir = TempDir::new("add");
        let seam = seam(&dir);

        seam.apply(&HostMutation::Add {
            endpoint: "10.0.0.5:11310".to_owned(),
            name: Some("Deadpool".to_owned()),
        })
        .expect("the add is accepted");

        let config = seam.store().load_or_default().expect("loads");
        assert_eq!(config.systems.len(), 1);
        assert_eq!(config.systems[0].host, "10.0.0.5");
        assert_eq!(config.systems[0].port, 11310);
        assert_eq!(config.systems[0].name.as_deref(), Some("Deadpool"));
    }

    #[test]
    fn a_refused_add_leaves_the_configuration_byte_identical() {
        let dir = TempDir::new("refused");
        seed(&dir, "10.0.0.5", 11310, Some("Deadpool"));
        let seam = seam(&dir);
        let before = bytes(&dir);

        let error = seam
            .apply(&HostMutation::Add {
                // A portless host: the CLI refuses it, so the GUI refuses it
                // with the CLI's reason.
                endpoint: "10.0.0.6".to_owned(),
                name: None,
            })
            .expect_err("a portless host is refused");

        assert!(!error.is_empty(), "a refusal carries a reason");
        assert_eq!(bytes(&dir), before, "a refused add changes nothing");
    }

    #[test]
    fn an_add_that_collides_with_a_configured_host_is_refused() {
        let dir = TempDir::new("collide");
        seed(&dir, "10.0.0.5", 11310, Some("Deadpool"));
        let seam = seam(&dir);
        let before = bytes(&dir);

        let error = seam
            .apply(&HostMutation::Add {
                endpoint: "10.0.0.5:11310".to_owned(),
                name: Some("Other".to_owned()),
            })
            .expect_err("a duplicate is refused");

        assert!(!error.is_empty());
        assert_eq!(bytes(&dir), before, "a refused add changes nothing");
        let config = seam.store().load_or_default().expect("loads");
        assert_eq!(
            config.systems[0].name.as_deref(),
            Some("Deadpool"),
            "the existing host keeps its name"
        );
    }

    #[test]
    fn a_rename_keeps_the_host_identity_and_its_place() {
        let dir = TempDir::new("rename");
        seed(&dir, "10.0.0.5", 11310, Some("Deadpool"));
        let seam = seam(&dir);
        let before_id = seam.store().load_or_default().expect("loads").systems[0]
            .id
            .clone();

        seam.apply(&HostMutation::Rename {
            endpoint: "10.0.0.5:11310".to_owned(),
            name: Some("Deadpool Prime".to_owned()),
        })
        .expect("the rename is accepted");

        let config = seam.store().load_or_default().expect("loads");
        assert_eq!(config.systems.len(), 1);
        assert_eq!(
            config.systems[0].id, before_id,
            "a rename is in place: the identity does not change"
        );
        assert_eq!(config.systems[0].name.as_deref(), Some("Deadpool Prime"));
        assert_eq!(config.systems[0].host, "10.0.0.5");
    }

    #[test]
    fn a_rename_to_an_empty_name_clears_the_configured_name() {
        let dir = TempDir::new("clear");
        seed(&dir, "10.0.0.5", 11310, Some("Deadpool"));
        let seam = seam(&dir);

        seam.apply(&HostMutation::Rename {
            endpoint: "10.0.0.5:11310".to_owned(),
            name: None,
        })
        .expect("clearing the name is accepted");

        let config = seam.store().load_or_default().expect("loads");
        assert_eq!(config.systems[0].name, None);
    }

    #[test]
    fn a_remove_takes_the_host_out_of_the_configuration() {
        let dir = TempDir::new("remove");
        seed(&dir, "10.0.0.5", 11310, None);
        let seam = seam(&dir);

        seam.apply(&HostMutation::Remove {
            endpoint: "10.0.0.5:11310".to_owned(),
        })
        .expect("the removal is accepted");

        let config = seam.store().load_or_default().expect("loads");
        assert!(config.systems.is_empty(), "the host is gone");
    }

    #[test]
    fn removing_a_host_that_is_not_configured_says_so() {
        let dir = TempDir::new("remove-missing");
        seed(&dir, "10.0.0.5", 11310, None);
        let seam = seam(&dir);
        let before = bytes(&dir);

        let error = seam
            .apply(&HostMutation::Remove {
                endpoint: "10.0.0.9:11310".to_owned(),
            })
            .expect_err("removing nothing is refused");

        assert!(
            error.contains("no configured host matches"),
            "the reason says what happened: {error}"
        );
        assert_eq!(bytes(&dir), before, "a refused removal changes nothing");
    }

    #[test]
    fn an_endpoint_change_replaces_the_host_and_changes_its_identity() {
        let dir = TempDir::new("replace");
        seed(&dir, "10.0.0.5", 11310, Some("Deadpool"));
        let seam = seam(&dir);
        let before_id = seam.store().load_or_default().expect("loads").systems[0]
            .id
            .clone();

        seam.apply(&HostMutation::Replace {
            from: "10.0.0.5:11310".to_owned(),
            to: "10.0.0.6:11310".to_owned(),
            name: Some("Deadpool".to_owned()),
        })
        .expect("the replacement is accepted");

        let config = seam.store().load_or_default().expect("loads");
        assert_eq!(config.systems.len(), 1, "there is one host, not two");
        assert_eq!(config.systems[0].host, "10.0.0.6");
        assert_ne!(
            config.systems[0].id, before_id,
            "a replacement changes the host's identity: the endpoint is part of it"
        );
    }

    #[test]
    fn a_refused_replacement_leaves_the_configuration_byte_identical() {
        let dir = TempDir::new("replace-refused");
        seed(&dir, "10.0.0.5", 11310, Some("Deadpool"));
        let seam = seam(&dir);

        // The new endpoint is already configured, so the replacement is
        // refused — and the old host must survive untouched.
        seam.apply(&HostMutation::Add {
            endpoint: "10.0.0.7:11310".to_owned(),
            name: None,
        })
        .expect("the second host is added");
        let before = bytes(&dir);

        let error = seam
            .apply(&HostMutation::Replace {
                from: "10.0.0.5:11310".to_owned(),
                to: "10.0.0.7:11310".to_owned(),
                name: None,
            })
            .expect_err("a replacement onto a configured endpoint is refused");

        assert!(!error.is_empty());
        assert_eq!(
            bytes(&dir),
            before,
            "a refused replacement changes nothing: the old host is still there"
        );
        let config = seam.store().load_or_default().expect("loads");
        assert_eq!(config.systems.len(), 2, "both hosts are still configured");
    }

    #[test]
    fn the_form_describes_an_add_when_it_has_an_endpoint() {
        let mut form = HostForm::adding();
        assert!(form.to_mutation().is_none(), "an empty form does nothing");
        form.endpoint = "10.0.0.5:11310".to_owned();
        form.name = "Deadpool".to_owned();
        assert_eq!(
            form.to_mutation(),
            Some(HostMutation::Add {
                endpoint: "10.0.0.5:11310".to_owned(),
                name: Some("Deadpool".to_owned()),
            })
        );
    }

    #[test]
    fn the_form_describes_a_rename_when_only_the_name_moved() {
        let form = HostForm::editing(HostTarget {
            id: "id-1".to_owned(),
            endpoint: "10.0.0.5:11310".to_owned(),
            name: Some("Deadpool".to_owned()),
        });
        assert!(
            form.to_mutation().is_none(),
            "an untouched form does nothing"
        );

        let mut renamed = form.clone();
        renamed.name = "Deadpool Prime".to_owned();
        assert_eq!(
            renamed.to_mutation(),
            Some(HostMutation::Rename {
                endpoint: "10.0.0.5:11310".to_owned(),
                name: Some("Deadpool Prime".to_owned()),
            })
        );
    }

    #[test]
    fn the_form_describes_a_replacement_when_the_endpoint_moved() {
        let mut form = HostForm::editing(HostTarget {
            id: "id-1".to_owned(),
            endpoint: "10.0.0.5:11310".to_owned(),
            name: Some("Deadpool".to_owned()),
        });
        form.endpoint = "10.0.0.6:11310".to_owned();
        assert!(form.endpoint_changed());
        assert_eq!(
            form.to_mutation(),
            Some(HostMutation::Replace {
                from: "10.0.0.5:11310".to_owned(),
                to: "10.0.0.6:11310".to_owned(),
                name: Some("Deadpool".to_owned()),
            })
        );
    }

    #[test]
    fn an_empty_name_in_the_form_clears_the_configured_name() {
        let mut form = HostForm::editing(HostTarget {
            id: "id-1".to_owned(),
            endpoint: "10.0.0.5:11310".to_owned(),
            name: Some("Deadpool".to_owned()),
        });
        form.name = String::new();
        assert_eq!(
            form.to_mutation(),
            Some(HostMutation::Rename {
                endpoint: "10.0.0.5:11310".to_owned(),
                name: None,
            })
        );
    }

    #[test]
    fn the_form_trims_what_the_operator_typed() {
        let mut form = HostForm::adding();
        form.endpoint = "  10.0.0.5:11310  ".to_owned();
        form.name = "  Deadpool  ".to_owned();
        assert_eq!(
            form.to_mutation(),
            Some(HostMutation::Add {
                endpoint: "10.0.0.5:11310".to_owned(),
                name: Some("Deadpool".to_owned()),
            })
        );
    }
}
