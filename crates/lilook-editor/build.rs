//! The build date, so the about box can say when this binary was made.
fn main() {
    // `SOURCE_DATE_EPOCH` where the environment sets it, so a reproducible build
    // stays reproducible; today otherwise.
    let date = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .map(days_since_epoch)
        .unwrap_or_else(|| {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            days_since_epoch(secs)
        });
    println!("cargo:rustc-env=LILOOK_BUILD_DATE={date}");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
}

/// `YYYY-MM-DD` from a unix timestamp, by the civil-from-days algorithm, so the
/// crate needs no date dependency for one string.
fn days_since_epoch(secs: i64) -> String {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
