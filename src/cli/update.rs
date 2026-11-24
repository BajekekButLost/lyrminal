use std::io::Write;

use clap::Args;

use crate::update::{self, CURRENT, Release};

#[derive(Args, Debug)]
pub struct UpdateArgs {
    #[arg(short, long, help = "Install without asking")]
    pub yes: bool,
    #[arg(long, help = "Only check: exit with 0 when up to date, 1 when a new release is out and 2 when the check fails")]
    pub check: bool,
    #[arg(long, conflicts_with = "check", help = "Build the release on this machine with cargo instead of downloading the prebuilt binary")]
    pub from_source: bool,
    #[arg(long = "version", value_name = "vX.Y.Z", conflicts_with = "check", help = "Install this release instead of the latest, even an older one")]
    pub version: Option<String>,
}

async fn pick(client: &reqwest::Client, args: &UpdateArgs) -> Result<Option<Release>, String> {
    let Some(v) = &args.version else {
        return update::check(client).await;
    };
    let tag = format!("v{}", v.trim().trim_start_matches(['v', 'V']));
    if update::parse_version(&tag).is_none() {
        return Err(format!("“{v}” is not a version like v1.2.3"));
    }
    match update::fetch(client, Some(&tag)).await? {
        Some(r) if r.version == CURRENT => {
            println!("Lyrminal v{CURRENT} is already installed");
            Ok(None)
        }
        Some(r) => Ok(Some(r)),
        None => Err(format!("there is no release {tag}")),
    }
}

async fn build(client: &reqwest::Client, release: &Release) -> Result<std::path::PathBuf, String> {
    eprintln!("Downloading and building Lyrminal v{} from source, this can take a few minutes", release.version);
    update::build_from_source(client, release, |_, _| {}).await
}

async fn check_only() -> i32 {
    match update::check(&crate::providers::http_client()).await {
        Ok(Some(r)) => {
            describe(&r);
            1
        }
        Ok(None) => {
            println!("Lyrminal v{CURRENT} is up to date");
            0
        }
        Err(e) => {
            eprintln!("lyrminal: could not check for updates: {e}");
            2
        }
    }
}

fn progress(done: u64, total: u64) {
    if total > 0 {
        eprint!("\rDownloading {}%", (done * 100 / total).min(100));
    } else {
        eprint!("\rDownloading {} KB", done / 1024);
    }
    let _ = std::io::stderr().flush();
}

fn ask(release: &Release, yes: bool) -> bool {
    if yes {
        return true;
    }
    if super::stdin_piped() {
        super::fail("pass -y to install without being asked");
    }
    super::confirm(&format!("Install Lyrminal v{}?", release.version))
}

async fn install_binary(client: &reqwest::Client, release: &Release) -> Result<std::path::PathBuf, String> {
    let mut shown = None;
    let result = update::install(client, release, |done, total| {
        let pct = (total > 0).then(|| done * 100 / total);
        if pct != shown || total == 0 {
            shown = pct;
            progress(done, total);
        }
    })
    .await;
    eprintln!();
    result
}

fn describe(release: &Release) {
    if update::is_newer(&release.version, CURRENT) {
        println!("Lyrminal v{} is out (you have v{CURRENT})", release.version);
    } else {
        println!("Lyrminal v{} (you have v{CURRENT})", release.version);
    }
    if !release.summary.is_empty() {
        println!("{}", release.summary);
    }
    println!("Release notes: {}", release.url);
}

pub async fn run(args: UpdateArgs) -> i32 {
    if args.check {
        return check_only().await;
    }
    if let Err(e) = update::install_path() {
        super::fail(e);
    }
    let client = crate::providers::http_client();
    let release = match pick(&client, &args).await {
        Ok(Some(r)) => r,
        Ok(None) if args.version.is_some() => return 0,
        Ok(None) => {
            println!("Lyrminal v{CURRENT} is up to date");
            return 0;
        }
        Err(e) => super::fail(format!("could not check for updates: {e}")),
    };
    describe(&release);
    let from_source = args.from_source || release.asset.is_none();
    if !args.from_source && from_source {
        eprintln!("lyrminal: there is no prebuilt binary for this system, so it will be built from source");
    }
    if from_source && !update::has_cargo() {
        super::fail("building needs cargo, install Rust from https://rustup.rs");
    }
    if !ask(&release, args.yes) {
        println!("Nothing was installed");
        return 1;
    }
    let installed = if from_source { build(&client, &release).await } else { install_binary(&client, &release).await };
    match installed {
        Ok(path) => {
            println!("Installed Lyrminal v{} at {}", release.version, path.display());
            println!("A running Lyrminal keeps the old version until you restart it");
            0
        }
        Err(e) => super::fail(format!("the update failed: {e}")),
    }
}
