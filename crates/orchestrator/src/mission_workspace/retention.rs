//! Snapshot retention policy + repository-scoped daily compaction check. Default:
//! 50 missions OR 7 days, whichever longer. Compaction deletes only
//! intermediate pre/post-integration tags. Final target-rollback anchors are
//! durable and remain available for History's Revert action.
//!
//! A one-shot check is scheduled whenever a mission starts in a canonical
//! repository. Checkpoints are keyed by repository, so activity in one project
//! cannot suppress cleanup in another. If the last successful pass is older
//! than 24 hours, compaction runs and advances the checkpoint.
//!
//! Adaptation note: the plan was written against `chrono`, which is
//! not a workspace dependency. We use unix-ms epochs throughout
//! (parsed from `git for-each-ref --format=...iso-strict` and from
//! `SystemTime::now()`) and only round-trip to RFC3339 strings for
//! the persisted `snapshot_compaction_state_by_repo.last_run_at` column.
//! The host crate also cannot reach `Repository`'s private SQLite
//! pool, so [`spawn_repo_compaction_if_due`] accepts a `Repository` clone
//! rather than a bare `Pool<Sqlite>` — same shape as the existing
//! [`crate::retention::RetentionGuard`] sweeper.

use crate::ids::rfc3339_from_unix_ms_pub;
use crate::repository::Repository;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MS_PER_DAY: u64 = 24 * 60 * 60 * 1000;

/// Snapshot retention envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub max_missions: u32,
    pub max_age_days: u32,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            max_missions: 50,
            max_age_days: 7,
        }
    }
}

/// Run one compaction pass. Returns the number of tags pruned.
///
/// Algorithm:
/// 1. List every `vigla/pre-merge/*` and `vigla/snap/*` tag with its creation
///    date. Durable `vigla/revert/*` anchors are deliberately excluded.
/// 2. Group by mission_id.
/// 3. Sort missions by latest tag date desc.
/// 4. Keep the first `max_missions` missions; for older missions,
///    additionally keep tags newer than `max_age_days`.
/// 5. Delete every tag not in the keep set.
///
/// `now_unix_ms` is the wall clock used to compute the age cutoff —
/// passed in rather than read internally so tests can simulate a
/// future "now" without time travel.
pub async fn compact_once(
    repo_root: &Path,
    policy: RetentionPolicy,
    now_unix_ms: u64,
) -> Result<u32, std::io::Error> {
    let cutoff_ms = now_unix_ms.saturating_sub(MS_PER_DAY * policy.max_age_days as u64);

    // List tags with their commit dates via `git for-each-ref`. We
    // use `iso-strict` (e.g. `2026-05-22T14:30:00+00:00`) — every
    // git ≥2.0 emits this canonical RFC3339 form, which our
    // [`parse_iso_strict_ms`] helper turns into a unix-ms epoch.
    //
    // The patterns use plain prefix matching (no trailing `*`): a
    // single `*` in a `git for-each-ref` pattern only matches one
    // ref segment, but `vigla/{kind}/{mid}/{n}` has TWO segments
    // below the kind. Bare prefixes get every ref under the prefix
    // at any depth, which is what we want.
    let raw = tokio::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args([
            "for-each-ref",
            "--format=%(refname:short)\t%(creatordate:iso-strict)",
            "refs/tags/vigla/pre-merge/",
            "refs/tags/vigla/snap/",
        ])
        .output()
        .await?;
    if !raw.status.success() {
        return Err(std::io::Error::other(format!(
            "git for-each-ref failed in {}: {}",
            repo_root.display(),
            String::from_utf8_lossy(&raw.stderr).trim()
        )));
    }
    let lines = String::from_utf8_lossy(&raw.stdout);

    let mut tags: Vec<(String, String, u64)> = Vec::new();
    for line in lines.lines() {
        let (name, date_str) = match line.split_once('\t') {
            Some(x) => x,
            None => continue,
        };
        let date_ms = match parse_iso_strict_ms(date_str.trim()) {
            Some(d) => d,
            None => continue,
        };
        // Mission id is the third segment: vigla/{kind}/{mid}/{n}
        let mid = name.split('/').nth(2).unwrap_or("").to_string();
        if mid.is_empty() {
            continue;
        }
        tags.push((name.to_string(), mid, date_ms));
    }

    // Group by mission_id; for each mission, record its latest tag date.
    use std::collections::HashMap;
    let mut latest_per_mission: HashMap<String, u64> = HashMap::new();
    for (_, mid, date) in &tags {
        latest_per_mission
            .entry(mid.clone())
            .and_modify(|d| {
                if *date > *d {
                    *d = *date;
                }
            })
            .or_insert(*date);
    }

    // Rank missions by latest date descending.
    let mut missions: Vec<(String, u64)> = latest_per_mission.into_iter().collect();
    missions.sort_by_key(|(_, d)| std::cmp::Reverse(*d));
    let kept_missions: std::collections::HashSet<String> = missions
        .iter()
        .take(policy.max_missions as usize)
        .map(|(m, _)| m.clone())
        .collect();

    // Build delete list.
    let mut to_delete = Vec::new();
    for (tag, mid, date) in &tags {
        let keep = kept_missions.contains(mid) || *date >= cutoff_ms;
        if !keep {
            to_delete.push(tag.clone());
        }
    }

    // Delete in one git invocation per tag (git tag -d takes
    // multiple args but using one-per-call keeps error reporting
    // clean; the loop is bounded by mission count × retention).
    let mut pruned = 0u32;
    for tag in &to_delete {
        let out = tokio::process::Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args(["tag", "-d", tag])
            .output()
            .await?;
        if !out.status.success() {
            return Err(std::io::Error::other(format!(
                "git tag -d {tag:?} failed in {}: {}",
                repo_root.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        pruned += 1;
    }

    Ok(pruned)
}

/// Run a repository-scoped compaction pass only when its last successful
/// checkpoint is at least 24 hours old. `Ok(None)` means it was not due.
pub async fn compact_if_due(
    repo_root: &Path,
    repo: &Repository,
    policy: RetentionPolicy,
    now_ms: u64,
) -> Result<Option<u32>, std::io::Error> {
    let repo_key = repo_root.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "repository path is not valid UTF-8: {}",
                repo_root.display()
            ),
        )
    })?;
    let last_ms = repo
        .last_compaction_run(repo_key)
        .await
        .map_err(std::io::Error::other)?
        .and_then(|timestamp| parse_rfc3339_ms(&timestamp));
    if last_ms.is_some_and(|last| now_ms.saturating_sub(last) < MS_PER_DAY) {
        return Ok(None);
    }

    let pruned = compact_once(repo_root, policy, now_ms).await?;
    repo.record_compaction_run(repo_key, &rfc3339_from_unix_ms_pub(now_ms), pruned)
        .await
        .map_err(std::io::Error::other)?;
    Ok(Some(pruned))
}

/// Schedule one fail-soft compaction check for the repository a mission just
/// opened. Failure leaves the old checkpoint intact so a later mission retries.
pub fn spawn_repo_compaction_if_due(
    repo_root: PathBuf,
    repo: Repository,
    policy: RetentionPolicy,
) -> tokio::task::JoinHandle<()> {
    crate::spawn_supervised("mission_workspace::compaction", async move {
        if let Err(error) = compact_if_due(&repo_root, &repo, policy, system_now_ms()).await {
            tracing::warn!(
                repo_root = %repo_root.display(),
                %error,
                "snapshot compaction failed; a later mission will retry"
            );
        }
    })
}

/// Current wall clock as unix milliseconds.
fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Parse `git`'s `iso-strict` date format: `YYYY-MM-DDTHH:MM:SS±HH:MM`.
/// Returns unix milliseconds, or `None` on a malformed string. The
/// timezone offset is honoured (the result is normalised to UTC).
fn parse_iso_strict_ms(s: &str) -> Option<u64> {
    // Minimum length is "2026-05-22T14:30:00Z" (20) or
    // "2026-05-22T14:30:00+00:00" (25). Tolerate either.
    let b = s.as_bytes();
    if b.len() < 20 || b[10] != b'T' {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    let hour: u64 = s[11..13].parse().ok()?;
    let minute: u64 = s[14..16].parse().ok()?;
    let second: u64 = s[17..19].parse().ok()?;

    // Parse timezone suffix. `Z`, `+HH:MM`, or `-HH:MM`.
    let tz_offset_minutes: i64 = if &s[19..] == "Z" || &s[19..] == "z" {
        0
    } else if b.len() >= 25 {
        let sign: i64 = match b[19] {
            b'+' => 1,
            b'-' => -1,
            _ => return None,
        };
        let tz_h: i64 = s[20..22].parse().ok()?;
        let tz_m: i64 = s[23..25].parse().ok()?;
        sign * (tz_h * 60 + tz_m)
    } else {
        return None;
    };

    // Howard Hinnant civil_from_days inverse (mirrors
    // crate::memory::scoring::parse_rfc3339_ms).
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * m as u64 + 2) / 5 + day as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let z = era * 146_097 + doe as i64 - 719_468;
    let local_secs = (z * 86_400) + (hour * 3600 + minute * 60 + second) as i64;
    let utc_secs = local_secs - tz_offset_minutes * 60;
    if utc_secs < 0 {
        return None;
    }
    Some(utc_secs as u64 * 1000)
}

/// Parse the RFC3339 timestamp written by [`rfc3339_now`].
/// Tolerates the canonical 24-char `YYYY-MM-DDTHH:MM:SS.mmmZ` shape
/// our code emits and the 20-char `Z`-suffixed shape some other
/// emitters use. Delegates to [`parse_iso_strict_ms`] for anything
/// non-millisecond. Returns `None` if the string isn't a recognised
/// shape.
fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    // Canonical `rfc3339_now()` shape: 24 chars, `.mmmZ` suffix.
    let b = s.as_bytes();
    if b.len() == 24 && b[10] == b'T' && b[23] == b'Z' {
        let year: i64 = s[0..4].parse().ok()?;
        let month: u32 = s[5..7].parse().ok()?;
        let day: u32 = s[8..10].parse().ok()?;
        let hour: u64 = s[11..13].parse().ok()?;
        let minute: u64 = s[14..16].parse().ok()?;
        let second: u64 = s[17..19].parse().ok()?;
        let ms: u64 = s[20..23].parse().ok()?;

        let y = if month <= 2 { year - 1 } else { year };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = (y - era * 400) as u64;
        let m = if month > 2 { month - 3 } else { month + 9 };
        let doy = (153 * m as u64 + 2) / 5 + day as u64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let z = era * 146_097 + doe as i64 - 719_468;
        let secs = (z * 86_400) + (hour * 3600 + minute * 60 + second) as i64;
        if secs < 0 {
            return None;
        }
        return Some(secs as u64 * 1000 + ms);
    }
    parse_iso_strict_ms(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission_workspace::tests::bootstrap_workspace_with_supervisor_branch;

    fn now_ms() -> u64 {
        system_now_ms()
    }

    #[tokio::test]
    async fn compact_once_keeps_recent_missions() {
        let (w, _td) = bootstrap_workspace_with_supervisor_branch().await;
        w.create_pre_merge_tag(0).await.unwrap();

        // With only one tag and default policy, nothing gets pruned.
        let pruned = compact_once(w.repo_root(), RetentionPolicy::default(), now_ms())
            .await
            .unwrap();
        assert_eq!(pruned, 0);
    }

    #[tokio::test]
    async fn compact_once_prunes_when_over_max_missions() {
        let (w, _td) = bootstrap_workspace_with_supervisor_branch().await;
        w.create_pre_merge_tag(0).await.unwrap();

        // Use a tight policy so even one tag exceeds budget. We
        // synthesise extra tags by creating sibling pre-merge tags
        // under a different mission id via raw git.
        for i in 0..3u32 {
            let tag = format!("vigla/pre-merge/old-mid-{i}/0");
            let sha = w
                .run_git(&["rev-parse", &w.supervisor_branch()])
                .await
                .unwrap()
                .trim()
                .to_string();
            w.run_git(&["tag", &tag, &sha]).await.unwrap();
        }

        // Pretend it's a month later so age-based retention also kicks in.
        let future_ms = now_ms() + 30 * MS_PER_DAY;
        let pruned = compact_once(
            w.repo_root(),
            RetentionPolicy {
                max_missions: 1,
                max_age_days: 0,
            },
            future_ms,
        )
        .await
        .unwrap();
        // 3 old-mid-* tags pruned (only the most recent mission kept).
        assert!(pruned >= 3, "expected at least 3 pruned, got {pruned}");
    }

    #[tokio::test]
    async fn compact_once_preserves_final_revert_anchors() {
        let (w, _td) = bootstrap_workspace_with_supervisor_branch().await;
        let sha = w
            .run_git(&["rev-parse", &w.supervisor_branch()])
            .await
            .unwrap();
        w.ensure_tag_at(&w.final_before_tag("main"), &sha)
            .await
            .unwrap();
        w.ensure_tag_at(&w.final_merged_tag("main"), &sha)
            .await
            .unwrap();

        let pruned = compact_once(
            w.repo_root(),
            RetentionPolicy {
                max_missions: 0,
                max_age_days: 0,
            },
            now_ms() + 30 * MS_PER_DAY,
        )
        .await
        .unwrap();

        assert_eq!(pruned, 0);
        assert_eq!(
            w.run_git(&["rev-parse", &w.final_before_tag("main")])
                .await
                .unwrap(),
            sha
        );
        assert_eq!(
            w.run_git(&["rev-parse", &w.final_merged_tag("main")])
                .await
                .unwrap(),
            sha
        );
    }

    #[tokio::test]
    async fn due_check_is_independent_for_each_repository() {
        let (first, _first_dir) = bootstrap_workspace_with_supervisor_branch().await;
        let (second, _second_dir) = bootstrap_workspace_with_supervisor_branch().await;
        let repository = Repository::open_in_memory().await.unwrap();
        let now = now_ms();

        assert_eq!(
            compact_if_due(
                first.repo_root(),
                &repository,
                RetentionPolicy::default(),
                now,
            )
            .await
            .unwrap(),
            Some(0)
        );
        assert_eq!(
            compact_if_due(
                first.repo_root(),
                &repository,
                RetentionPolicy::default(),
                now + 1,
            )
            .await
            .unwrap(),
            None
        );
        assert_eq!(
            compact_if_due(
                second.repo_root(),
                &repository,
                RetentionPolicy::default(),
                now + 1,
            )
            .await
            .unwrap(),
            Some(0),
            "a recent pass in one repository must not suppress another"
        );
    }

    #[test]
    fn parse_iso_strict_round_trip_utc() {
        // `Z` suffix.
        let ms = parse_iso_strict_ms("2026-05-22T14:30:00Z").unwrap();
        // Sanity check: 2026-05-22T14:30:00Z is well past 2025-01-01.
        let start_2025 = parse_iso_strict_ms("2025-01-01T00:00:00Z").unwrap();
        assert!(ms > start_2025);
    }

    #[test]
    fn parse_iso_strict_honors_offset() {
        // 2026-05-22T22:30:00+08:00 == 2026-05-22T14:30:00Z.
        let a = parse_iso_strict_ms("2026-05-22T22:30:00+08:00").unwrap();
        let b = parse_iso_strict_ms("2026-05-22T14:30:00Z").unwrap();
        assert_eq!(a, b);
        // Negative offset.
        let c = parse_iso_strict_ms("2026-05-22T09:30:00-05:00").unwrap();
        let d = parse_iso_strict_ms("2026-05-22T14:30:00Z").unwrap();
        assert_eq!(c, d);
    }

    #[test]
    fn parse_rfc3339_ms_handles_canonical_now_shape() {
        let s = crate::ids::rfc3339_now();
        let ms = parse_rfc3339_ms(&s).expect("parsable");
        // Within 5s of system clock.
        let live = system_now_ms();
        assert!(
            ms.abs_diff(live) < 5_000,
            "parsed {ms} vs live {live} differ by too much"
        );
    }
}
