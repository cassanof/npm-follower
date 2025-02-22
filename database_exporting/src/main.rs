use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use time::{format_description::well_known::iso8601, OffsetDateTime, PrimitiveDateTime};
use users::{get_current_uid, get_user_by_uid};

const FORMATTER: iso8601::Iso8601<
    {
        iso8601::Config::DEFAULT
            .set_time_precision(iso8601::TimePrecision::Hour {
                decimal_digits: Some(unsafe { std::num::NonZeroU8::new_unchecked(8) }),
            })
            .encode()
    },
> = iso8601::Iso8601;

fn load_hf_token() -> String {
    let mut tokens_path = std::env::current_dir().unwrap();
    tokens_path.pop();
    tokens_path.push(".secret.env");
    dotenvy::from_filename(".secret.env").expect("failed to load .secret.env.");
    std::env::var("HF_TOKEN").expect("HF_TOKEN env var not set")
}

fn main() -> io::Result<()> {
    let user = get_user_by_uid(get_current_uid()).unwrap();
    if user.name().to_string_lossy() != "postgres" {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "This program must be run as the postgres user",
        ));
    }

    let hf_token = load_hf_token();

    let tmp_backup_base: PathBuf =
        "/var/lib/postgresql/exports-npm-follower/metadata_external-tmp".into();
    let local_backup_base: PathBuf =
        "/var/lib/postgresql/exports-npm-follower/metadata_external".into();

    let now = OffsetDateTime::now_utc();
    let now_str = now.format(&FORMATTER).unwrap();

    let tmp_backup_dir = tmp_backup_base.join("db_export");
    let tmp_backup_tar = tmp_backup_base.join(format!("{}.tar", now_str));

    let tmp_backup_dir_relative = "db_export";
    let tmp_backup_tar_relative = format!("{}.tar", &now_str);

    let local_backup_tar = local_backup_base.join(format!("{}.tar", now_str));
    let local_backup_latest = local_backup_base.join("latest.tar");

    let exclude_table_params = vec![
        "change_log",
        "diff_log",
        "download_tasks",
        "internal_diff_log_state",
        "internal_state",
        "possibly_malicious_packages",
        "security_replaced_versions",
        // "packages",
        // "versions",
        // "dependencies",
        // "ghsa",
        // "vulnerabilities",
        // "downloaded_tarballs",
    ];

    let mut cmd = Command::new("pg_dump");
    cmd.args(["-j", "2", "-F", "d", "-f"]).arg(&tmp_backup_dir);

    cmd.arg("--schema=public");
    for param in exclude_table_params {
        cmd.arg("-T").arg(param);
    }

    cmd.arg("--no-acl").arg("npm_data");

    println!(
        "Current time: {} ({})",
        now.format(&iso8601::Iso8601::DEFAULT).unwrap(),
        now_str
    );
    println!(
        "Starting export of external metadata to: {}",
        tmp_backup_dir.display()
    );
    println!("+ {:?}", cmd);
    cmd.spawn()?.wait()?;

    println!("Exporting redis data");
    let mut export_script_path = std::env::current_dir()?;
    export_script_path.pop();
    export_script_path.push("blob_idx_server");
    export_script_path.push("export_redis.sh");
    assert!(Command::new("sudo")
        .arg(export_script_path)
        .current_dir(&tmp_backup_dir)
        .status()?
        .success());

    assert!(Command::new("sudo")
        .args(["chown", "postgres:postgres", "redis.zip"])
        .current_dir(&tmp_backup_dir)
        .status()?
        .success());

    println!("Creating tar file of dump");
    Command::new("tar")
        .current_dir(&tmp_backup_base)
        .arg("cvf")
        .arg(tmp_backup_tar_relative)
        .arg(tmp_backup_dir_relative)
        .spawn()?
        .wait()?;

    Command::new("chmod")
        .arg("g-w")
        .arg(&tmp_backup_tar)
        .spawn()?
        .wait()?;

    println!("Cleaning up dump dir");
    Command::new("rm")
        .arg("-rf")
        .arg(&tmp_backup_dir)
        .spawn()?
        .wait()?;

    println!("Moving tar file to final location");
    Command::new("mv")
        .arg(&tmp_backup_tar)
        .arg(&local_backup_tar)
        .spawn()?
        .wait()?;

    println!("Updating latest symlink");
    Command::new("ln")
        .arg("-sf")
        .arg(&local_backup_tar)
        .arg(&local_backup_latest)
        .spawn()?
        .wait()?;

    let tarball_paths = read_tarballs_in_dir(local_backup_base);
    if tarball_paths.len() >= 6 {
        if let Some(oldest_tarball_path) = find_oldest_tarball(&tarball_paths) {
            println!("Deleting the oldest tarball: {:?}", oldest_tarball_path);
            fs::remove_file(oldest_tarball_path).expect("Failed to delete the oldest tarball");
        } else {
            println!("No tarballs found in the directory");
        }
    }

    println!("Completed export of external metadata. Total size:");
    Command::new("du")
        .arg("-Lh")
        .arg(&local_backup_latest)
        .spawn()?
        .wait()?;

    println!("Uploading to huggingface.co");

    let mut upload_script_path = std::env::current_dir()?;
    upload_script_path.push("hf_upload.py");
    let repo_path_dated = Path::new("db_dumps").join(local_backup_tar.file_name().unwrap());
    let repo_path_latest = Path::new("db_dumps").join(local_backup_latest.file_name().unwrap());

    const REPO_NAME: &str = "donald-pinckney/npm-follower-data";
    assert!(Command::new("python3")
        .args([
            upload_script_path.as_os_str(),
            OsStr::new(&hf_token),
            OsStr::new(REPO_NAME),
            local_backup_tar.as_os_str(),
            repo_path_dated.as_os_str(),
            local_backup_latest.as_os_str(),
            repo_path_latest.as_os_str(),
        ])
        .status()?
        .success());

    Ok(())
}

fn read_tarballs_in_dir<P: AsRef<Path>>(dir_path: P) -> Vec<PathBuf> {
    let mut tarball_paths = vec![];
    for entry in fs::read_dir(dir_path).expect("Failed to read directory") {
        let entry = entry.expect("Failed to get directory entry");
        let file_type = entry.file_type().unwrap();
        let path = entry.path();
        if file_type.is_file() && path.extension().unwrap_or_default() == "tar" {
            tarball_paths.push(path);
        }
    }
    tarball_paths
}

fn find_oldest_tarball(tarball_paths: &[PathBuf]) -> Option<PathBuf> {
    tarball_paths
        .iter()
        .filter_map(|p| extract_date_from_tarball_name(p).map(|d| (p, d)))
        .min_by_key(|(_p, d)| *d)
        .map(|(p, _d)| p)
        .cloned()
}

fn extract_date_from_tarball_name(path: &Path) -> Option<PrimitiveDateTime> {
    let filename = path.file_stem().unwrap().to_str().unwrap();
    PrimitiveDateTime::parse(filename, &FORMATTER).ok()
}
