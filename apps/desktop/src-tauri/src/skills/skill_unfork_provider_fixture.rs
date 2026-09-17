use super::super::skill_process::{run_controlled_prepared_command_output, AddOperationControl};
use skill_studio_core::{
    skill_backup_reservation::ReservedManagedSource,
    skill_unfork_preparation::DotagentsRuntimeRecord,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub(super) struct ProviderFixture {
    node: PathBuf,
    provider: super::super::skill_unfork_provider::StagedUnforkProvider,
    pub(super) record: DotagentsRuntimeRecord,
}

impl ProviderFixture {
    pub(super) fn load() -> Self {
        let root = PathBuf::from(
            std::env::var_os("SKILL_STUDIO_RUNTIME_FIXTURE")
                .expect("explicit copied runtime fixture required"),
        )
        .canonicalize()
        .unwrap();
        let input: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("input.json")).unwrap()).unwrap();
        let node = PathBuf::from(input["node"].as_str().unwrap())
            .canonicalize()
            .unwrap();
        let record: DotagentsRuntimeRecord =
            serde_json::from_slice(&fs::read(root.join("verified-record.json")).unwrap()).unwrap();
        let result = Self {
            node: node.clone(),
            provider: super::super::skill_unfork_provider::StagedUnforkProvider::bind(
                &root,
                &node,
                record.clone(),
            )
            .unwrap(),
            record,
        };
        result.verify();
        result
    }

    fn verify(&self) {
        self.provider
            .verify_materialized_runtime(&AddOperationControl::bounded_default())
            .unwrap();
    }

    pub(super) fn stage(
        &self,
        reservation: &ReservedManagedSource<'_>,
        request: &skill_studio_core::skill_dotagents_ledger::DotagentsReinstallRequest,
        document: &[u8],
    ) {
        self.verify();
        let stage = reservation.stage_path().unwrap();
        let cache = reservation.cache_path().unwrap();
        let home = stage.join("home");
        let agents = home.join(".agents");
        fs::create_dir_all(&agents).unwrap();
        let source = request;
        assert_eq!(source.repo(), "owner/repo");
        assert_eq!(source.path(), "skills/alpha");
        fs::write(
            agents.join("agents.toml"),
            "version = 1\n[trust]\ngithub_orgs = ['owner']\n",
        )
        .unwrap();
        let repository = stage.join("fixture-repository");
        let selected = repository.join(source.path());
        fs::create_dir_all(&selected).unwrap();
        fs::write(selected.join("SKILL.md"), document).unwrap();
        fs::write(selected.join("resource.txt"), b"retained cache resource").unwrap();
        std::os::unix::fs::symlink("resource.txt", selected.join("link.txt")).unwrap();
        let sibling = agents.join("skills/beta/SKILL.md");
        fs::create_dir_all(sibling.parent().unwrap()).unwrap();
        fs::write(&sibling, b"untouched sibling").unwrap();
        for directory in ["tmp", "config", "data", "xdg-state", "xdg-cache"] {
            fs::create_dir(stage.join(directory)).unwrap();
        }
        let control = AddOperationControl::bounded_default();
        let configure = |program: &Path| {
            let mut command = Command::new(program);
            command
                .env_clear()
                .current_dir(&stage)
                .env(
                    "PATH",
                    format!("{}:/usr/bin:/bin", self.node.parent().unwrap().display()),
                )
                .env("HOME", &home)
                .env("DOTAGENTS_HOME", &agents)
                .env("DOTAGENTS_STATE_DIR", &cache)
                .env("TMPDIR", stage.join("tmp"))
                .env("XDG_CONFIG_HOME", stage.join("config"))
                .env("XDG_DATA_HOME", stage.join("data"))
                .env("XDG_STATE_HOME", stage.join("xdg-state"))
                .env("XDG_CACHE_HOME", stage.join("xdg-cache"))
                .env("CI", "1")
                .env("NO_COLOR", "1")
                .env("LANG", "en_US.UTF-8")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
                .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z");
            command
        };
        let run =
            |command| run_controlled_prepared_command_output(command, &control, 64 * 1024).unwrap();
        for args in [
            vec!["init", "--initial-branch=main"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-m",
                "Fixture input",
            ],
        ] {
            let mut command = configure(Path::new("/usr/bin/git"));
            command.current_dir(&repository).args(args);
            run(command);
        }
        let mut head = configure(Path::new("/usr/bin/git"));
        head.current_dir(&repository).args(["rev-parse", "HEAD"]);
        let head = String::from_utf8(run(head)).unwrap().trim().to_owned();
        let canary = stage.parent().unwrap().join("provider-outside-canary");
        fs::write(&canary, b"outside unchanged").unwrap();
        self.provider
            .assert_stage_denies_outside_write(reservation, &canary, &control)
            .unwrap();
        fs::write(
            stage.join("config/gitconfig"),
            format!(
                "[url \"{}\"]\n\tinsteadOf = https://github.com/owner/repo\n[protocol \"file\"]\n\tallow = always\n",
                reqwest::Url::from_directory_path(&repository).unwrap(),
            ),
        )
        .unwrap();
        let output = self
            .provider
            .run_staged_add(reservation, request, &control)
            .unwrap();
        assert_eq!(fs::read(&canary).unwrap(), b"outside unchanged");
        assert_eq!(fs::read(sibling).unwrap(), b"untouched sibling");
        let resolved = request
            .reinstalled_source(
                &fs::read_to_string(agents.join("agents.lock")).unwrap(),
                &fs::read_to_string(agents.join("agents.toml")).unwrap(),
            )
            .unwrap();
        assert_eq!(resolved.commit(), head);
        assert_eq!(resolved.declared_ref(), source.declared_ref());
        self.verify();
        println!("Real provider {} / Node {} staged commit {} with outside write denied and sibling preserved; {} output bytes", self.record.provider_version, self.record.node_version, head, output.len());
    }
}
