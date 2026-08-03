use codestory_workspace::same_workspace_path;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

#[test]
fn tmp_measure_same_workspace_path_scan_cost() {
    let dir = PathBuf::from(std::env::var("TMP_PROBE_DIR").expect("TMP_PROBE_DIR"));
    let n_excl: usize = std::env::var("TMP_PROBE_EXCL").unwrap().parse().unwrap();
    let n_cited: usize = std::env::var("TMP_PROBE_CITED").unwrap().parse().unwrap();
    let ex_dir = dir.join("ex");
    let ci_dir = dir.join("ci");
    fs::create_dir_all(&ex_dir).unwrap();
    fs::create_dir_all(&ci_dir).unwrap();
    let excluded: Vec<PathBuf> = (0..n_excl)
        .map(|i| {
            let p = ex_dir.join(format!("excluded_{i}.json"));
            if !p.exists() { fs::write(&p, b"x").unwrap(); }
            p
        })
        .collect();
    let cited: Vec<PathBuf> = (0..n_cited)
        .map(|i| {
            let p = ci_dir.join(format!("cited_{i}.rs"));
            if !p.exists() { fs::write(&p, b"x").unwrap(); }
            p
        })
        .collect();
    // warm
    for p in excluded.iter().chain(cited.iter()) { let _ = fs::metadata(p); }

    let start = Instant::now();
    let mut hits = 0;
    for c in &cited {
        if excluded.iter().any(|e| same_workspace_path(e, c)) { hits += 1; }
    }
    let el = start.elapsed();
    println!(
        "REAL_FN exclusions={n_excl} cited={n_cited} stats={} hits={hits} elapsed_ms={:.3}",
        2 * n_excl * n_cited,
        el.as_secs_f64() * 1000.0
    );
}
