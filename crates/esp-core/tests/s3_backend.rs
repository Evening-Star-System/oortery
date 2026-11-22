//! Real-backend integration test: exercise the engine against a live S3 (MinIO), not a local
//! path, so the "any cloud" claim is proven end to end through Oortery's own code paths.
//!
//! Opt-in. Skipped unless `OORTERY_S3_IT=1` and the S3 endpoint/creds are in the environment,
//! so the normal `cargo test` run (and CI) never needs a server. To run it:
//!
//!   OORTERY_S3_IT=1 OORTERY_S3_ENDPOINT=http://127.0.0.1:9100 \
//!   OORTERY_S3_KEY=oortery OORTERY_S3_SECRET=oortery-test-secret \
//!   cargo test -p esp-core --test s3_backend -- --nocapture
//!
//! It configures its own throwaway remote in a temp rclone config, so it does not touch the
//! machine's real rclone remotes.

use esp_core::{
    create_remote, init_repo, list_snapshots, mount, restic_path, run_backup, unmount, BackupJob,
    Endpoint, Job, Rcd, SyncMode,
};
use std::time::{Duration, Instant};

fn run_sync(rcd: &Rcd, job: &Job, group: &str) {
    let jobid = rcd.start_job(job, group).expect("start job");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let st = rcd.job_status(jobid).expect("job status");
        if st.finished {
            assert!(st.success, "sync failed: {}", st.error);
            return;
        }
        assert!(Instant::now() < deadline, "sync did not finish in time");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn s3_sync_and_backup_roundtrip() {
    if std::env::var("OORTERY_S3_IT").is_err() {
        eprintln!("OORTERY_S3_IT not set; skipping S3 integration test");
        return;
    }
    let endpoint = std::env::var("OORTERY_S3_ENDPOINT").expect("OORTERY_S3_ENDPOINT");
    let key = std::env::var("OORTERY_S3_KEY").expect("OORTERY_S3_KEY");
    let secret = std::env::var("OORTERY_S3_SECRET").expect("OORTERY_S3_SECRET");
    let bucket = std::env::var("OORTERY_S3_BUCKET").unwrap_or_else(|_| "oortery-it".into());
    // Per-run prefix so repeat runs against a persistent MinIO stay isolated (a reused restic
    // repo would otherwise accumulate snapshots and make the count non-deterministic).
    let prefix = format!("{bucket}/run-{}", std::process::id());

    let base = std::env::temp_dir().join(format!("oortery-s3it-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    std::env::set_var("RCLONE_CONFIG", base.join("rclone.conf"));
    std::env::set_var("XDG_CONFIG_HOME", base.join("cfg"));

    // Configure the S3 remote exactly as the add-remote flow would.
    create_remote(
        "s3it",
        "s3",
        &[
            ("provider".into(), "Minio".into()),
            ("access_key_id".into(), key),
            ("secret_access_key".into(), secret),
            ("endpoint".into(), endpoint),
        ],
    )
    .expect("create s3 remote");

    let rcd = Rcd::start().expect("start rcd");

    // A couple of local files to protect.
    let src = base.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("alpha.txt"), b"alpha").unwrap();
    std::fs::write(src.join("beta.txt"), b"beta").unwrap();

    // --- Write path: local -> S3 (the sync creates the bucket on S3) ---
    let up = Job {
        id: "s3-up".into(),
        name: "s3 up".into(),
        src: Endpoint::local(src.to_string_lossy().into_owned()),
        dst: Endpoint::remote("s3it", format!("{prefix}/sync")),
        mode: SyncMode::Copy,
        schedule: esp_core::Schedule::Manual,
        last_run: None,
        at: None,
    };
    run_sync(&rcd, &up, "it/s3-up");

    // --- Read path: S3 -> a fresh local dir; prove the bytes made the full round trip ---
    let down = base.join("down");
    std::fs::create_dir_all(&down).unwrap();
    let dn = Job {
        id: "s3-down".into(),
        name: "s3 down".into(),
        src: Endpoint::remote("s3it", format!("{prefix}/sync")),
        dst: Endpoint::local(down.to_string_lossy().into_owned()),
        mode: SyncMode::Copy,
        schedule: esp_core::Schedule::Manual,
        last_run: None,
        at: None,
    };
    run_sync(&rcd, &dn, "it/s3-down");
    assert_eq!(
        std::fs::read(down.join("alpha.txt")).unwrap(),
        b"alpha",
        "alpha.txt did not round-trip through S3"
    );
    assert!(
        down.join("beta.txt").exists(),
        "beta.txt missing after S3 round-trip"
    );

    // --- Mount path: expose the S3 prefix as a local folder and read a file through it ---
    if std::path::Path::new("/dev/fuse").exists() {
        let point = base.join("mnt");
        std::fs::create_dir_all(&point).unwrap();
        let point_s = point.to_string_lossy().into_owned();
        mount(rcd.port(), &format!("s3it:{prefix}/sync"), &point_s).expect("mount S3");
        let mut visible = false;
        for _ in 0..50 {
            if point.join("alpha.txt").exists() {
                visible = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(visible, "alpha.txt not visible through the S3 mount");
        unmount(rcd.port(), &point_s).expect("unmount S3");
    } else {
        eprintln!("no /dev/fuse; skipping S3 mount check");
    }

    // --- restic backup over rclone -> S3, with the integrity gate against real object storage ---
    if restic_path().is_some() {
        let bjob = BackupJob {
            id: "s3-backup".into(),
            name: "s3 backup".into(),
            sources: vec![src.to_string_lossy().into_owned()],
            repo: Endpoint::remote("s3it", format!("{prefix}/restic")),
            excludes: Vec::new(),
            schedule: esp_core::Schedule::Manual,
            last_run: None,
            at: None,
        };
        esp_core::save_password(&bjob.id, "it-pw").expect("save pw");
        init_repo(&bjob).expect("init restic repo on S3");
        // run_backup only returns Ok after `restic check` passes against the S3 repo.
        let snap = run_backup(&bjob, |_| {}).expect("backup + verify on S3");
        assert!(!snap.is_empty(), "no snapshot id from S3 backup");
        assert_eq!(list_snapshots(&bjob).expect("snapshots").len(), 1);
    }

    drop(rcd);
    std::env::remove_var("RCLONE_CONFIG");
    std::env::remove_var("XDG_CONFIG_HOME");
    let _ = std::fs::remove_dir_all(&base);
}
