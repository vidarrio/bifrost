//! How custom apworlds get into a season. Either from the
//! curated catalog — sysadmin-vetted, usable anywhere — or as an upload,
//! which is installed right away when the instance auto-approves uploads, and
//! otherwise waits in the season's `pending/` (where AP never loads it) until
//! a season moderator or a sysadmin approves it. Signups whose YAML needs a
//! pending world are held, then checked once it's approved.
//!
//! Every install goes through [`ap_adapter::install_world`], which pins the
//! hash; nothing here bypasses the pin check that guards generation.

use crate::id::Id;
use std::io::Read;
use std::path::PathBuf;

use crate::ap_adapter::{self, Pin};
use crate::config::{self, SeasonDirs};
use crate::db::{Approval, CatalogWorld, Db, WorldRequest};

/// File names an upload may not take: the world Bifrost ships (Universal
/// Tracker), which is trusted by content. The catalog's common games' apworlds
/// are reserved too (see [`upload_file`]).
const RESERVED: &[&str] = &["tracker"];

/// The name an uploaded apworld installs as, from the uploaded file's name.
pub fn world_file(upload_name: &str) -> Result<String, &'static str> {
    let stem = std::path::Path::new(upload_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("world");
    let base = crate::text::slug(stem);
    if RESERVED.contains(&base.to_lowercase().as_str()) {
        return Err("That apworld name is reserved. Rename the file and try again.");
    }
    Ok(format!("{base}.apworld"))
}

/// [`world_file`] for an upload, which can't take the name of an apworld one
/// of the catalog's common games ships either.
pub fn upload_file(upload_name: &str) -> Result<String, &'static str> {
    let file = world_file(upload_name)?;
    if crate::common::apworld_files()
        .iter()
        .any(|w| w.eq_ignore_ascii_case(&file))
    {
        return Err("That apworld name is reserved. Rename the file and try again.");
    }
    Ok(file)
}

/// `.apworld`s are zips; a quick magic-byte check rejects obvious mistakes.
pub fn looks_like_apworld(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && &bytes[..2] == b"PK"
}

/// What an apworld says about itself (`<world>/archipelago.json`; worlds
/// built before AP 0.6 have none).
#[derive(Debug, Default, PartialEq)]
pub struct Manifest {
    pub game: String,
    pub version: String,
}

pub fn manifest(bytes: &[u8]) -> Manifest {
    manifest_from(std::io::Cursor::new(bytes))
}

/// [`manifest`] of an apworld on disk, reading only what it needs.
pub fn manifest_of(path: &std::path::Path) -> Manifest {
    std::fs::File::open(path).map_or_else(|_| Manifest::default(), manifest_from)
}

/// The manifest Archipelago itself reads (`APContainer.read_contents`): the
/// zip's root `archipelago.json`, else the first entry whose name ends in
/// `archipelago.json`, at any depth. Bifrost has to see the same game AP will.
fn manifest_from<R: Read + std::io::Seek>(zip: R) -> Manifest {
    let read = || -> Option<Manifest> {
        let mut zip = zip::ZipArchive::new(zip).ok()?;
        let name = if zip.index_for_name("archipelago.json").is_some() {
            "archipelago.json".to_string()
        } else {
            (0..zip.len())
                .filter_map(|i| zip.name_for_index(i)?.ok().map(|n| n.into_owned()))
                .find(|n| n.ends_with("archipelago.json"))?
        };
        let mut json = String::new();
        zip.by_name(&name)
            .ok()?
            .take(64 * 1024)
            .read_to_string(&mut json)
            .ok()?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        let s = |k: &str| match &v[k] {
            serde_json::Value::String(s) => s.chars().take(80).collect(),
            serde_json::Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        Some(Manifest {
            game: s("game"),
            version: s("world_version"),
        })
    };
    read().unwrap_or_default()
}

pub fn catalog_file(sha256: &str) -> PathBuf {
    config::get()
        .catalog_dir()
        .join(format!("{sha256}.apworld"))
}

/// Where an upload ended up.
#[derive(Debug, PartialEq)]
pub enum Placed {
    /// Installed and usable now (`catalog`: it matched a catalog entry).
    Installed { catalog: bool },
    /// Waiting for approval.
    Pending,
    /// Its game is one of the season's core games: nothing installed, the
    /// signup uses the core game (AP would ignore the upload anyway).
    Core { game: String },
    /// Another upload of its game is waiting for approval in this season: the
    /// signup waits for that one (`file`).
    Waiting { game: String, file: String },
    /// Its game is already installed in this season: the signup uses that
    /// (`file`), and nothing changes. A season keeps one world per game.
    Season {
        game: String,
        file: String,
        version: String,
        offered: Option<String>,
    },
    /// Its game is in the catalog: the catalog's version was installed
    /// instead (as `file`). `offered`: the upload was newer, so it was kept
    /// for a sysadmin to promote as the catalog's replacement.
    Ours {
        game: String,
        file: String,
        version: String,
        offered: Option<String>,
    },
    /// A newer version of a game the season has, installed in its place
    /// (`old`: the version it replaced). The season's signups for the game
    /// are being checked against it.
    Replaced {
        game: String,
        file: String,
        version: String,
        old: String,
        offered: bool,
    },
    /// A newer version of a game the season has, waiting for approval; once
    /// approved it replaces the season's (`old`). `unknown`: one of the two
    /// declares no version that can be compared, so a moderator decides.
    Replacing {
        game: String,
        version: String,
        old: String,
        offered: bool,
        unknown: bool,
    },
}

/// Whether a season can still take a newer version of a game: not once it's
/// been generated, as its seed was made with the version it has.
async fn can_replace(db: &Db, season: Id) -> bool {
    use crate::db::State;
    db.season(season)
        .await
        .is_some_and(|s| matches!(s.state.0, State::Draft | State::Signup))
}

/// Install `bytes` as `file` in place of `old` (the season's world for
/// `game`), then hold the season's signups for the game and check each
/// against the new version: those that pass go ahead, the rest are flagged.
/// One lock per season, held by generation while it moves the season to
/// generating and by a replacement from its state check until the signups
/// are held: a season is never generated with a world its signups weren't
/// checked against.
pub async fn season_lock(season: Id) -> tokio::sync::OwnedMutexGuard<()> {
    static LOCKS: std::sync::Mutex<
        std::collections::BTreeMap<Id, std::sync::Arc<tokio::sync::Mutex<()>>>,
    > = std::sync::Mutex::new(std::collections::BTreeMap::new());
    let lock = LOCKS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entry(season)
        .or_default()
        .clone();
    lock.lock_owned().await
}

async fn replace_world(
    db: &Db,
    season: Id,
    old: &str,
    file: &str,
    bytes: Vec<u8>,
    pin: Pin,
    game: &str,
) -> Result<(), String> {
    let _lock = season_lock(season).await;
    if !can_replace(db, season).await {
        return Err(format!(
            "The season has been generated, so it keeps the {game} its seed was made with."
        ));
    }
    let dirs = config::get().season(season);
    if old != file && ap_adapter::world_pins(&dirs).contains_key(file) {
        return Err(format!(
            "A different {file} is already installed this season. It has to be removed (from the season admin page) before this can take its place."
        ));
    }
    // The new one first: if that fails, the season still has the old.
    install(dirs, file.to_string(), bytes, pin).await?;
    if old != file
        && let Err(e) = crate::generate::delete_apworld(season, old).await
    {
        tracing::warn!("season {season}: removing {old}, replaced by {file}: {e}");
    }
    let n = db.hold_game(season, game, old, file).await;
    tracing::info!("season {season}: {game} replaced by {file}; re-checking {n} signup(s)");
    Ok(())
}

/// Check the signups held for `file`, in the background.
fn recheck_later(db: &Db, season: Id, file: &str) {
    let (db, file) = (db.clone(), file.to_string());
    tokio::spawn(async move { recheck_held(&db, season, &file).await });
}

/// The world a season already has for `game` (Bifrost's own aside): its file
/// name, hash and version. A season keeps one world per game.
pub fn installed_for_game(dirs: &SeasonDirs, game: &str) -> Option<(String, String, String)> {
    if game.trim().is_empty() {
        return None;
    }
    ap_adapter::world_pins(dirs)
        .into_iter()
        .filter(|(_, pin)| !pin.shipped)
        .find_map(|(file, pin)| {
            let m = manifest_of(&dirs.worlds.join(&file));
            (m.game == game).then_some((file, pin.sha256, m.version))
        })
}

/// The custom games a season has installed (Bifrost's own aside): game,
/// file and the version its manifest declares, by game name.
pub fn installed_games(dirs: &SeasonDirs) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = ap_adapter::world_pins(dirs)
        .into_iter()
        .filter(|(_, pin)| !pin.shipped)
        .filter_map(|(file, _)| {
            let m = manifest_of(&dirs.worlds.join(&file));
            (!m.game.is_empty()).then_some((m.game, file, m.version))
        })
        .collect();
    out.sort_by_key(|a| a.0.to_lowercase());
    out
}

/// A dotted version as numbers ("v1.10.2" → [1, 10, 2]), if it is one.
fn version_parts(v: &str) -> Option<Vec<u64>> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let parts: Option<Vec<u64>> = v.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty())
}

/// Whether version `a` is newer than `b`, when both are dotted versions.
pub fn version_newer(a: &str, b: &str) -> bool {
    match (version_parts(a), version_parts(b)) {
        (Some(mut a), Some(mut b)) => {
            let n = a.len().max(b.len());
            a.resize(n, 0);
            b.resize(n, 0);
            a > b
        }
        _ => false,
    }
}

/// The catalog's entry for a game (its newest, should there be several).
pub fn catalog_for_game<'a>(catalog: &'a [CatalogWorld], game: &str) -> Option<&'a CatalogWorld> {
    let game = game.trim();
    if game.is_empty() {
        return None;
    }
    catalog
        .iter()
        // Exactly, as Archipelago matches games.
        .filter(|w| w.game == game)
        .reduce(|best, w| {
            if version_newer(&w.version, &best.version) {
                w
            } else {
                best
            }
        })
}

/// A world kept for a sysadmin to add to the catalog (or dismiss): a newer
/// version of a catalog game that a player uploaded, or a world a season
/// moderator approved that the catalog doesn't have (as new). Kept as
/// files in `$BIFROST_DATA/catalog/offers/`: `<sha256>.apworld` and its
/// `<sha256>.json`.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Offer {
    pub sha256: String,
    pub file: String,
    pub game: String,
    pub version: String,
    /// The catalog version it would replace.
    pub replaces: String,
    pub uploaded_by: String,
    pub uploaded_at: i64,
    pub season: Id,
    /// Who approved it in its season, if a moderator did.
    #[serde(default)]
    pub approved_by: String,
}

fn offers_dir() -> PathBuf {
    config::get().catalog_dir().join("offers")
}

fn is_sha(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The most offers kept at once: each holds an upload (up to 32 MiB), and
/// anyone who can sign up can make one.
const MAX_OFFERS: usize = 20;

/// Keep an upload as an offer: one per player and game (a newer one replaces
/// theirs), and none past [`MAX_OFFERS`]. False when it wasn't kept.
async fn offer_store(offer: &Offer, bytes: &[u8]) -> Result<bool, String> {
    for o in offers() {
        if o.game == offer.game && o.uploaded_by == offer.uploaded_by {
            if !version_newer(&offer.version, &o.version) {
                return Ok(false);
            }
            offer_remove(&o.sha256).await;
        }
    }
    if offers().len() >= MAX_OFFERS {
        tracing::warn!(
            "not keeping {}'s {} {}: {MAX_OFFERS} offers are waiting on the catalog page",
            offer.uploaded_by,
            offer.game,
            offer.version
        );
        return Ok(false);
    }
    let dir = offers_dir();
    let res = async {
        tokio::fs::create_dir_all(&dir).await?;
        crate::atomic::write_async(
            dir.join(format!("{}.apworld", offer.sha256)),
            bytes.to_vec(),
        )
        .await?;
        let json = serde_json::to_vec(offer).expect("offer serializes");
        crate::atomic::write_async(dir.join(format!("{}.json", offer.sha256)), json).await
    };
    res.await
        .map(|()| true)
        .map_err(|e| format!("keeping the newer version: {e}"))
}

/// Every offer waiting for a sysadmin, newest first.
pub fn offers() -> Vec<Offer> {
    let mut out: Vec<Offer> = std::fs::read_dir(offers_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
        .collect();
    out.sort_by_key(|o: &Offer| std::cmp::Reverse(o.uploaded_at));
    out
}

async fn offer_remove(sha256: &str) {
    let dir = offers_dir();
    let _ = tokio::fs::remove_file(dir.join(format!("{sha256}.apworld"))).await;
    let _ = tokio::fs::remove_file(dir.join(format!("{sha256}.json"))).await;
}

/// Promote an offer into the catalog, replacing the game's current entry.
pub async fn offer_promote(db: &Db, sha256: &str, by: &str) -> Result<Added, String> {
    if !is_sha(sha256) {
        return Err("No such offer.".into());
    }
    let offer = offers()
        .into_iter()
        .find(|o| o.sha256 == sha256)
        .ok_or("No such offer.")?;
    let bytes = tokio::fs::read(offers_dir().join(format!("{sha256}.apworld")))
        .await
        .map_err(|e| format!("reading the offer: {e}"))?;
    if crate::text::sha256_hex(&bytes) != sha256 {
        return Err("The offer's file doesn't match its hash; dismiss it.".into());
    }
    if let Some(cat) = catalog_for_game(&db.catalog().await, &offer.game)
        && !version_newer(&offer.version, &cat.version)
    {
        offer_remove(sha256).await;
        return Err(format!(
            "The catalog has {} {} already, no older than this offer ({}); it's been dismissed.",
            cat.game, cat.version, offer.version
        ));
    }
    let note = format!("uploaded by {}", offer.uploaded_by);
    let added = catalog_add(db, &offer.file, &bytes, "", "", &note, "", by).await?;
    offer_remove(sha256).await;
    Ok(added)
}

/// A world a season moderator (not a sysadmin) just approved: offer it to the
/// sysadmins for the catalog, unless the catalog has it, or its game at least
/// as new, already.
pub async fn offer_approved(db: &Db, season: Id, req: &WorldRequest, by: &str) {
    if db.catalog_world(&req.sha256).await.is_some() {
        return;
    }
    let path = config::get().season(season).worlds.join(&req.file);
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return;
    };
    if crate::text::sha256_hex(&bytes) != req.sha256 {
        return;
    }
    let m = manifest(&bytes);
    // Without a game it can't be told from others, or replace anything.
    if m.game.is_empty() {
        return;
    }
    let cat = catalog_for_game(&db.catalog().await, &m.game).cloned();
    if let Some(cat) = &cat
        && !version_newer(&m.version, &cat.version)
    {
        return;
    }
    // Already offered (as a newer version of a catalog game): say who
    // approved it.
    if let Some(mut o) = offers().into_iter().find(|o| o.sha256 == req.sha256) {
        o.approved_by = by.to_string();
        let json = serde_json::to_vec(&o).expect("offer serializes");
        if let Err(e) =
            crate::atomic::write_async(offers_dir().join(format!("{}.json", o.sha256)), json).await
        {
            tracing::warn!("noting who approved an offer: {e}");
        }
        return;
    }
    let offer = Offer {
        sha256: req.sha256.clone(),
        file: req.file.clone(),
        game: m.game,
        version: m.version,
        replaces: cat.map(|c| c.version).unwrap_or_default(),
        uploaded_by: req.uploaded_by.clone(),
        uploaded_at: req.uploaded_at,
        season,
        approved_by: by.to_string(),
    };
    match offer_store(&offer, &bytes).await {
        Ok(true) => {
            tracing::info!(
                "{by} approved {} in season {season}: offered to sysadmins for the catalog",
                req.file
            );
            crate::notify::catalog_offer(db, &offer).await;
        }
        Ok(false) => {}
        Err(e) => tracing::warn!("{e}"),
    }
}

/// An offer's file, for a sysadmin to vet: its name and bytes.
pub async fn offer_file(sha256: &str) -> Result<(String, Vec<u8>), String> {
    if !is_sha(sha256) {
        return Err("No such offer.".into());
    }
    let offer = offers()
        .into_iter()
        .find(|o| o.sha256 == sha256)
        .ok_or("No such offer.")?;
    let bytes = tokio::fs::read(offers_dir().join(format!("{sha256}.apworld")))
        .await
        .map_err(|e| format!("reading the offer: {e}"))?;
    Ok((offer.file, bytes))
}

/// Drop an offer.
pub async fn offer_dismiss(sha256: &str) -> Result<(), String> {
    if !is_sha(sha256) || !offers().iter().any(|o| o.sha256 == sha256) {
        return Err("No such offer.".into());
    }
    offer_remove(sha256).await;
    Ok(())
}

/// Refuse to swap a world out from under the YAMLs that use it: a different
/// file under an installed name has to be removed by a moderator first.
fn installed_conflict(dirs: &SeasonDirs, file: &str, sha256: &str) -> Result<bool, String> {
    match ap_adapter::world_pins(dirs).get(file) {
        Some(p) if p.sha256 == sha256 => Ok(true),
        Some(_) => Err(format!(
            "A different {file} is already installed this season. Ask a moderator to remove it first."
        )),
        None => Ok(false),
    }
}

async fn install(dirs: SeasonDirs, file: String, bytes: Vec<u8>, pin: Pin) -> Result<(), String> {
    tokio::task::spawn_blocking(move || ap_adapter::install_world(&dirs, &file, &bytes, pin))
        .await
        .map_err(|e| e.to_string())?
}

/// Place an uploaded apworld in `season`: installed if it's in the catalog or
/// the instance auto-approves, otherwise pending.
pub async fn place_upload(
    db: &Db,
    season: Id,
    file: &str,
    bytes: Vec<u8>,
    uploader: &str,
) -> Result<Placed, String> {
    let dirs = config::get().season(season);
    let sha = crate::text::sha256_hex(&bytes);
    // A game the server already has: use the server's version. The season's
    // own copy if it has one (a season keeps one world per game), else the
    // catalog's; a core game needs nothing, as AP always loads its own. An
    // upload newer than the catalog's version is offered to sysadmins either
    // way.
    let m = manifest(&bytes);
    let in_catalog = db.catalog_world(&sha).await.is_some();
    let cat = if in_catalog {
        None
    } else {
        catalog_for_game(&db.catalog().await, &m.game).cloned()
    };
    let offered = match &cat {
        Some(cat) if version_newer(&m.version, &cat.version) => {
            let offer = Offer {
                sha256: sha.clone(),
                file: file.to_string(),
                game: cat.game.clone(),
                version: m.version.clone(),
                replaces: cat.version.clone(),
                uploaded_by: uploader.to_string(),
                uploaded_at: crate::clock::now(),
                season,
                approved_by: String::new(),
            };
            match offer_store(&offer, &bytes).await {
                Ok(true) => {
                    crate::notify::catalog_offer(db, &offer).await;
                    tracing::info!(
                        "{uploader} uploaded {} {} (the catalog has {}): offered to sysadmins",
                        cat.game,
                        m.version,
                        cat.version
                    );
                    Some(m.version.clone())
                }
                Ok(false) => None,
                // The signup goes ahead on the server's version either way.
                Err(e) => {
                    tracing::warn!("{e}");
                    None
                }
            }
        }
        _ => None,
    };
    // The season's own world for the game (this very file, or another). A
    // newer version replaces it until the season is generated: at once if
    // it's the catalog's or the server auto-approves, else once approved.
    let mut replaces = None;
    // When either version can't be read, which is newer can't be told: a
    // moderator decides (see `approve_with`'s override).
    let mut unknown = false;
    if let Some((have, have_sha, version)) = installed_for_game(&dirs, &m.game) {
        unknown = version_parts(&m.version).is_none() || version_parts(&version).is_none();
        if have_sha == sha
            || !(unknown || version_newer(&m.version, &version))
            || !can_replace(db, season).await
        {
            return Ok(Placed::Season {
                game: m.game,
                file: have,
                version,
                offered,
            });
        }
        replaces = Some((have, version));
    }
    if let Some((old_file, old)) = replaces {
        let base = Pin {
            uploaded_by: uploader.to_string(),
            uploaded_at: crate::clock::now(),
            ..Pin::default()
        };
        // The catalog has this game at least as new: use the catalog's.
        if let Some(cat) = &cat
            && !unknown
            && version_newer(&cat.version, &old)
            && !version_newer(&m.version, &cat.version)
        {
            let bytes = tokio::fs::read(catalog_file(&cat.sha256))
                .await
                .map_err(|e| format!("reading the catalog copy: {e}"))?;
            if crate::text::sha256_hex(&bytes) != cat.sha256 {
                return Err(
                    "The catalog copy doesn't match its hash. Ask a sysadmin to re-add it.".into(),
                );
            }
            let pin = Pin {
                uploaded_by: uploader.to_string(),
                uploaded_at: crate::clock::now(),
                approved_by: cat.added_by.clone(),
                catalog: true,
                ..Pin::default()
            };
            replace_world(db, season, &old_file, &cat.file, bytes, pin, &m.game).await?;
            recheck_later(db, season, &cat.file);
            return Ok(Placed::Replaced {
                game: m.game,
                file: cat.file.clone(),
                version: cat.version.clone(),
                old,
                offered: offered.is_some(),
            });
        }
        // Only the catalog's own file replaces it at once. Anything else
        // waits for a moderator, even where uploads are auto-approved: it
        // changes the code everyone signed up with the game plays.
        let now = if unknown {
            None
        } else {
            db.catalog_world(&sha).await.map(|cat| Pin {
                catalog: true,
                approved_by: cat.added_by,
                ..base
            })
        };
        if let Some(pin) = now {
            replace_world(db, season, &old_file, file, bytes, pin, &m.game).await?;
            recheck_later(db, season, file);
            return Ok(Placed::Replaced {
                game: m.game,
                file: file.to_string(),
                version: m.version,
                old,
                offered: offered.is_some(),
            });
        }
        // Waits for approval, as a new world would (below), unless another
        // upload of the game already does.
        if let Some(req) = db.world_request(season, file).await {
            return if req.sha256 == sha {
                Ok(Placed::Replacing {
                    game: m.game,
                    version: m.version,
                    old,
                    offered: offered.is_some(),
                    unknown,
                })
            } else {
                Err(format!(
                    "Another version of {file} is already waiting for approval."
                ))
            };
        }
        if let Some(req) = db
            .world_requests(season)
            .await
            .into_iter()
            .find(|r| r.game == m.game)
        {
            return Ok(Placed::Waiting {
                game: m.game,
                file: req.file,
            });
        }
        store_request(db, season, &dirs, file, &sha, &m.game, &bytes, uploader).await?;
        return Ok(Placed::Replacing {
            game: m.game,
            version: m.version,
            old,
            offered: offered.is_some(),
            unknown,
        });
    }
    if let Some(cat) = cat {
        let installed = install_from_catalog(db, season, &cat.sha256, uploader).await?;
        return Ok(Placed::Ours {
            game: cat.game,
            file: installed,
            version: cat.version,
            offered,
        });
    }
    if !in_catalog {
        let core = crate::games::cached(&crate::apinstall::for_dirs(&dirs));
        if !m.game.is_empty() && core.is_some_and(|c| c.iter().any(|g| g.game == m.game)) {
            return Ok(Placed::Core { game: m.game });
        }
    }
    if installed_conflict(&dirs, file, &sha)? {
        return Ok(Placed::Installed { catalog: false });
    }
    let base = Pin {
        uploaded_by: uploader.to_string(),
        uploaded_at: crate::clock::now(),
        ..Pin::default()
    };
    if let Some(cat) = db.catalog_world(&sha).await {
        let pin = Pin {
            catalog: true,
            approved_by: cat.added_by,
            ..base
        };
        install(dirs, file.to_string(), bytes, pin).await?;
        return Ok(Placed::Installed { catalog: true });
    }
    if let Some(req) = db.world_request(season, file).await {
        return if req.sha256 == sha {
            Ok(Placed::Pending)
        } else {
            Err(format!(
                "Another version of {file} is already waiting for approval."
            ))
        };
    }
    // Another upload of the game waiting for approval: a season keeps one
    // world per game, so this signup waits for that one.
    if !m.game.is_empty()
        && let Some(req) = db
            .world_requests(season)
            .await
            .into_iter()
            .find(|r| r.game == m.game)
    {
        return Ok(Placed::Waiting {
            game: m.game,
            file: req.file,
        });
    }
    match db.approval().await {
        Approval::Auto => {
            install(dirs, file.to_string(), bytes, base).await?;
            Ok(Placed::Installed { catalog: false })
        }
        Approval::Manual => {
            store_request(db, season, &dirs, file, &sha, &m.game, &bytes, uploader).await?;
            Ok(Placed::Pending)
        }
    }
}

/// Keep an upload to wait for a moderator's approval.
#[allow(clippy::too_many_arguments)]
async fn store_request(
    db: &Db,
    season: Id,
    dirs: &SeasonDirs,
    file: &str,
    sha: &str,
    game: &str,
    bytes: &[u8],
    uploader: &str,
) -> Result<(), String> {
    tokio::fs::create_dir_all(&dirs.pending)
        .await
        .map_err(|e| format!("pending dir: {e}"))?;
    tokio::fs::write(dirs.pending.join(file), bytes)
        .await
        .map_err(|e| format!("storing the upload: {e}"))?;
    let req = WorldRequest {
        file: file.to_string(),
        sha256: sha.to_string(),
        game: game.to_string(),
        uploaded_by: uploader.to_string(),
        uploaded_at: crate::clock::now(),
    };
    db.add_world_request(season, &req)
        .await
        .map_err(|e| e.to_string())?;
    crate::notify::upload_waiting(db, season, uploader, file, game).await;
    Ok(())
}

/// Install a catalog world into `season`. Returns the file name it took.
pub async fn install_from_catalog(
    db: &Db,
    season: Id,
    sha256: &str,
    by: &str,
) -> Result<String, String> {
    let cat = db
        .catalog_world(sha256)
        .await
        .ok_or("That world isn't in the catalog.")?;
    let bytes = tokio::fs::read(catalog_file(&cat.sha256))
        .await
        .map_err(|e| format!("reading the catalog copy: {e}"))?;
    if crate::text::sha256_hex(&bytes) != cat.sha256 {
        return Err("The catalog copy doesn't match its hash. Ask a sysadmin to re-add it.".into());
    }
    let dirs = config::get().season(season);
    // A season keeps one world per game: one it has already is what its
    // signups use (the catalog may have moved on since).
    if let Some((have, _, _)) = installed_for_game(&dirs, &cat.game) {
        return Ok(have);
    }
    if installed_conflict(&dirs, &cat.file, &cat.sha256)? {
        return Ok(cat.file);
    }
    let pin = Pin {
        uploaded_by: by.to_string(),
        uploaded_at: crate::clock::now(),
        approved_by: cat.added_by.clone(),
        catalog: true,
        ..Pin::default()
    };
    install(dirs, cat.file.clone(), bytes, pin).await?;
    Ok(cat.file)
}

/// The catalog's version of an installed world's game, if it's newer than
/// the season's copy (`file`): what [`update_from_catalog`] would install.
pub fn catalog_update<'a>(
    dirs: &SeasonDirs,
    catalog: &'a [CatalogWorld],
    file: &str,
) -> Option<&'a CatalogWorld> {
    let pin = ap_adapter::world_pins(dirs).get(file).cloned()?;
    if pin.shipped || pin.common {
        return None;
    }
    let m = manifest_of(&dirs.worlds.join(file));
    let cat = catalog_for_game(catalog, &m.game)?;
    // Newer, or the season's copy declares no version to compare with.
    let newer = version_newer(&cat.version, &m.version) || version_parts(&m.version).is_none();
    (cat.sha256 != pin.sha256 && newer).then_some(cat)
}

/// Replace a season's world with the catalog's newer version of its game,
/// and check the season's signups for it again. Before generation only.
pub async fn update_from_catalog(
    db: &Db,
    season: Id,
    file: &str,
    by: &str,
) -> Result<String, String> {
    let dirs = config::get().season(season);
    let catalog = db.catalog().await;
    let cat = catalog_update(&dirs, &catalog, file)
        .cloned()
        .ok_or("The catalog has no newer version of that world's game.")?;
    let bytes = tokio::fs::read(catalog_file(&cat.sha256))
        .await
        .map_err(|e| format!("reading the catalog copy: {e}"))?;
    if crate::text::sha256_hex(&bytes) != cat.sha256 {
        return Err("The catalog copy doesn't match its hash. Ask a sysadmin to re-add it.".into());
    }
    let pin = Pin {
        uploaded_by: by.to_string(),
        uploaded_at: crate::clock::now(),
        approved_by: cat.added_by.clone(),
        catalog: true,
        ..Pin::default()
    };
    replace_world(db, season, file, &cat.file, bytes, pin, &cat.game).await?;
    recheck_later(db, season, &cat.file);
    Ok(format!(
        "Updated {} to the catalog's {}. Its signups are being checked against it again.",
        cat.game, cat.version
    ))
}

/// Approve a pending upload: install it, then check the signups held for it
/// (in the background — each check can take a while). `force`: a moderator's override, replacing the
/// season's world for the game even when the upload doesn't declare a newer
/// version (say, neither declares one).
pub async fn approve_with(
    db: &Db,
    season: Id,
    file: &str,
    by: &str,
    force: bool,
) -> Result<WorldRequest, String> {
    let dirs = config::get().season(season);
    let req = db
        .world_request(season, file)
        .await
        .ok_or("Nothing is waiting under that name.")?;
    let path = dirs.pending.join(&req.file);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| format!("reading the upload: {e}"))?;
    if crate::text::sha256_hex(&bytes) != req.sha256 {
        return Err(format!(
            "{file} changed since it was uploaded. Reject it and ask for a new upload."
        ));
    }
    // A season keeps one world per game. A newer version replaces the one
    // it has (until it's generated); otherwise the one it has wins.
    if let Some((have, have_sha, version)) = installed_for_game(&dirs, &req.game)
        && have_sha != req.sha256
    {
        if !force && !version_newer(&manifest(&bytes).version, &version) {
            return Err(format!(
                "This season already has {} installed ({have}), and this upload doesn't declare a newer version. Reject it (signups held for it can use that), or replace the season's anyway.",
                req.game
            ));
        }
        // The catalog's own file (held only because its version couldn't be
        // compared) installs as the catalog's.
        let pin = match db.catalog_world(&req.sha256).await {
            Some(cat) => Pin {
                uploaded_by: req.uploaded_by.clone(),
                uploaded_at: req.uploaded_at,
                approved_by: cat.added_by,
                catalog: true,
                ..Pin::default()
            },
            None => Pin {
                uploaded_by: req.uploaded_by.clone(),
                uploaded_at: req.uploaded_at,
                approved_by: by.to_string(),
                ..Pin::default()
            },
        };
        replace_world(db, season, &have, &req.file, bytes, pin, &req.game).await?;
        let _ = tokio::fs::remove_file(&path).await;
        db.take_world_request(season, file).await;
        recheck_later(db, season, &req.file);
        return Ok(req);
    }
    if !installed_conflict(&dirs, &req.file, &req.sha256)? {
        let pin = Pin {
            uploaded_by: req.uploaded_by.clone(),
            uploaded_at: req.uploaded_at,
            approved_by: by.to_string(),
            ..Pin::default()
        };
        install(dirs, req.file.clone(), bytes, pin).await?;
    }
    let _ = tokio::fs::remove_file(&path).await;
    db.take_world_request(season, file).await;
    let (db, file) = (db.clone(), req.file.clone());
    tokio::spawn(async move { recheck_held(&db, season, &file).await });
    Ok(req)
}

/// Reject a pending upload; its held signups say so until they're replaced or removed.
pub async fn reject(db: &Db, season: Id, file: &str, by: &str) -> Result<WorldRequest, String> {
    let req = db
        .take_world_request(season, file)
        .await
        .ok_or("Nothing is waiting under that name.")?;
    let dirs = config::get().season(season);
    let _ = tokio::fs::remove_file(dirs.pending.join(&req.file)).await;
    // A newer version of a game the season has: its signups use the one it
    // has instead, if their YAMLs pass that.
    if let Some((have, _, _)) = installed_for_game(&dirs, &req.game) {
        db.rehold(season, &req.file, &have).await;
        let db = db.clone();
        tokio::spawn(async move { recheck_held(&db, season, &have).await });
        return Ok(req);
    }
    let why = format!(
        "{} was rejected by {by}: replace the YAML with an installed game's, or remove this signup",
        req.file
    );
    let mut flagged = Vec::new();
    for (id, yaml) in db.held_submissions(season, &req.file).await {
        if matches!(db.set_held_err(id, &yaml, &why).await, Ok(true)) {
            crate::notify::signup_flagged(db, season, id, &why).await;
            if let Some((_, slot)) = db.submission_owner(id).await {
                flagged.push(slot);
            }
        }
    }
    // The other moderators: whoever rejected it knows.
    let rejecter = db.account_by_name(by).await.map(|a| a.id);
    crate::notify::signups_flagged_for_mods(db, season, &flagged, &why, rejecter).await;
    Ok(req)
}

/// A signup just stored as held for `file`: if its world was decided while that was
/// going on, nothing else will look at it, so settle it now.
pub async fn settle_held(db: &Db, season: Id, id: Id, yaml: &str, file: &str) {
    if db.world_request(season, file).await.is_some() {
        return;
    }
    if config::get().season(season).worlds.join(file).is_file() {
        let (db, file) = (db.clone(), file.to_string());
        tokio::spawn(async move { recheck_held(&db, season, &file).await });
    } else {
        let why = format!(
            "{file} was rejected: replace the YAML with an installed game's, or remove this signup"
        );
        if matches!(db.set_held_err(id, yaml, &why).await, Ok(true)) {
            crate::notify::signup_flagged(db, season, id, &why).await;
            if let Some((owner, slot)) = db.submission_owner(id).await {
                crate::notify::signups_flagged_for_mods(db, season, &[slot], &why, Some(owner))
                    .await;
            }
        }
    }
}

/// A signup is one game, so its file holds one player.
pub const ONE_GAME_PER_FILE: &str = "That YAML holds several players. Sign up one game per file.";

/// Check every signup held for `file` now that it's installed: those that
/// pass go ahead (with the name and game the check read); the rest keep
/// the reason.
pub async fn recheck_held(db: &Db, season: Id, file: &str) {
    let dirs = config::get().season(season);
    let mut flagged: Vec<(String, String, i64)> = Vec::new();
    for (id, yaml_path) in db.held_submissions(season, file).await {
        let read = match config::get().data_file(&yaml_path) {
            Some(p) => tokio::fs::read_to_string(p).await,
            None => Err(std::io::Error::other("not in the data directory")),
        };
        let outcome = match read {
            Err(e) => Err(format!("the stored YAML is gone ({e})")),
            Ok(yaml) => match check_with_retries(&dirs, &yaml).await {
                // No verdict: `stale_holds` picks it up again.
                Err(e) => Err(format!(
                    "couldn't be checked ({e}); it's tried again before generating"
                )),
                Ok(r) if !r.ok => Err(format!("its YAML didn't pass: {}", r.errors.join("; "))),
                Ok(r) if r.players.len() > 1 => Err(ONE_GAME_PER_FILE.to_string()),
                Ok(r) => match r.players.first() {
                    None => Err("couldn't read a player from its YAML".to_string()),
                    Some(p) => slot_name_of_player(p)
                        .map(|name| (name, p.game.clone().unwrap_or_else(|| "?".into()))),
                },
            },
        };
        let res = match outcome {
            Ok((name, game)) => db
                .release_submission(id, &yaml_path, &name, &game)
                .await
                .map_err(|_| format!("the slot name {name} is already signed up")),
            Err(e) => Err(e),
        };
        if let Err(e) = res {
            tracing::info!("held signup {id} in season {season} still held: {e}");
            let changed = matches!(db.set_held_err(id, &yaml_path, &e).await, Ok(true));
            // A check that couldn't run is tried again: no verdict to tell.
            if changed && !e.starts_with("couldn't be checked") {
                crate::notify::signup_flagged(db, season, id, &e).await;
                if let Some((owner, slot)) = db.submission_owner(id).await {
                    flagged.push((slot, e, owner));
                }
            }
        }
    }
    // The moderators hear once, however many failed.
    match flagged.as_slice() {
        [] => {}
        // One: its own reason, and not to its owner, who hears as the player.
        [(slot, why, owner)] => {
            let slots = std::slice::from_ref(slot);
            crate::notify::signups_flagged_for_mods(db, season, slots, why, Some(*owner)).await
        }
        many => {
            let slots: Vec<String> = many.iter().map(|(s, _, _)| s.clone()).collect();
            let why =
                format!("checked against {file}, each is held; the season admin page says why");
            crate::notify::signups_flagged_for_mods(db, season, &slots, &why, None).await;
        }
    }
}

/// A YAML check, retried a few times when the checker couldn't run it (busy
/// queue, timeout) — those aren't verdicts on the YAML.
async fn check_with_retries(
    dirs: &config::SeasonDirs,
    yaml: &str,
) -> Result<crate::checker::CheckResult, String> {
    let mut wait = std::time::Duration::from_secs(15);
    for _ in 0..3 {
        match crate::checker::check_yaml(dirs, yaml).await {
            Ok(r) => return Ok(r),
            Err(e) => tracing::info!(
                "re-checking a held signup: {e}; retrying in {}s",
                wait.as_secs()
            ),
        }
        tokio::time::sleep(wait).await;
        wait *= 2;
    }
    crate::checker::check_yaml(dirs, yaml).await
}

/// Finish re-checks that were cut short (a restart mid-loop, a check that
/// couldn't run): held signups whose world is installed and no longer waits
/// for approval. Run at startup and before generating.
pub async fn recheck_stale(db: &Db, season: Id) {
    let dirs = config::get().season(season);
    for file in db.stale_holds(season).await {
        if dirs.worlds.join(&file).is_file() {
            recheck_held(db, season, &file).await;
        }
    }
}

/// A catalog addition, and the entries for the same game it replaced.
pub struct Added {
    pub world: CatalogWorld,
    pub replaced: Vec<CatalogWorld>,
}

impl Added {
    /// What to tell the sysadmin.
    pub fn message(&self) -> String {
        let w = &self.world;
        let what = if w.game.is_empty() {
            w.file.clone()
        } else {
            format!("{} {}", w.game, w.version).trim().to_string()
        };
        let mut m = format!("Added {what} to the catalog.");
        for old in &self.replaced {
            m.push_str(&format!(
                " It replaces {}; seasons that installed that keep it.",
                if old.version.is_empty() {
                    old.file.clone()
                } else {
                    format!("version {}", old.version)
                }
            ));
        }
        m
    }
}

/// The most a catalog note (the sysadmins' or the public one) holds, in characters.
pub const NOTE_MAX: usize = 200;

/// A note as stored: one trimmed line of at most [`NOTE_MAX`] characters.
pub fn clean_note(note: &str) -> String {
    note.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(NOTE_MAX)
        .collect()
}

/// Add an apworld to the catalog (or re-describe it). `game`/`version`
/// default to what its manifest says. The catalog holds one version of a game
/// (it's what a player's upload of that game is swapped for), so this
/// replaces any other entry for the same game; seasons that installed it keep
/// their copy. A replacement with no public note keeps the old entry's, as
/// that's about the game rather than one file.
#[allow(clippy::too_many_arguments)]
pub async fn catalog_add(
    db: &Db,
    file: &str,
    bytes: &[u8],
    game: &str,
    version: &str,
    note: &str,
    public_note: &str,
    by: &str,
) -> Result<Added, String> {
    let sha = crate::text::sha256_hex(bytes);
    let m = manifest(bytes);
    let pick = |typed: &str, read: String| {
        let t = typed.trim();
        if t.is_empty() {
            read
        } else {
            t.chars().take(80).collect()
        }
    };
    let w = CatalogWorld {
        file: file.to_string(),
        game: pick(game, m.game),
        version: pick(version, m.version),
        note: clean_note(note),
        public_note: clean_note(public_note),
        added_by: by.to_string(),
        added_at: crate::clock::now(),
        sha256: sha,
    };
    let store = async {
        tokio::fs::create_dir_all(config::get().catalog_dir()).await?;
        crate::atomic::write_async(catalog_file(&w.sha256), bytes.to_vec()).await
    };
    if let Err(e) = store.await {
        return Err(format!("storing the catalog copy: {e}"));
    }
    let mut w = w;
    let catalog = db.catalog().await;
    let olds: Vec<CatalogWorld> = if w.game.is_empty() {
        Vec::new()
    } else {
        catalog
            .iter()
            .filter(|old| old.sha256 != w.sha256 && old.game == w.game)
            .cloned()
            .collect()
    };
    // Replacing a game is for a newer version: going back (or sideways, to
    // the same version) takes removing the entry first, on purpose.
    if let Some(old) = olds.iter().find(|o| {
        version_parts(&o.version).is_some()
            && version_parts(&w.version).is_some()
            && !version_newer(&w.version, &o.version)
    }) {
        return Err(format!(
            "The catalog already has {} {}, and this is {}, which isn't newer. Remove that entry first to replace it with this one.",
            old.game, old.version, w.version
        ));
    }
    // A note left blank keeps the game's: this entry's own when it's being
    // re-described, else the one of an entry it replaces.
    if w.public_note.is_empty()
        && let Some(old) = catalog
            .iter()
            .filter(|o| o.sha256 == w.sha256 || olds.iter().any(|x| x.sha256 == o.sha256))
            .find(|o| !o.public_note.is_empty())
    {
        w.public_note = old.public_note.clone();
    }
    db.catalog_add(&w).await.map_err(|e| e.to_string())?;
    for old in &olds {
        catalog_remove(db, &old.sha256).await?;
    }
    // Offers no newer than the game's catalog version now are moot.
    for o in offers() {
        if o.sha256 == w.sha256 || (o.game == w.game && !version_newer(&o.version, &w.version)) {
            offer_remove(&o.sha256).await;
        }
    }
    Ok(Added {
        world: w,
        replaced: olds,
    })
}

/// Add a world already installed in a season to the catalog.
pub async fn catalog_add_installed(
    db: &Db,
    season: Id,
    file: &str,
    by: &str,
) -> Result<Added, String> {
    let dirs = config::get().season(season);
    let pin = ap_adapter::world_pins(&dirs)
        .get(file)
        .cloned()
        .ok_or("That world isn't installed.")?;
    let bytes = tokio::fs::read(dirs.worlds.join(file))
        .await
        .map_err(|e| e.to_string())?;
    if crate::text::sha256_hex(&bytes) != pin.sha256 {
        return Err(format!("{file} changed on disk since it was installed."));
    }
    catalog_add(db, file, &bytes, "", "", "", "", by).await
}

/// Drop a catalog entry. Seasons that already installed it keep their copy.
pub async fn catalog_remove(db: &Db, sha256: &str) -> Result<(), String> {
    if !db.catalog_remove(sha256).await.map_err(|e| e.to_string())? {
        return Err("That world isn't in the catalog.".into());
    }
    let _ = tokio::fs::remove_file(catalog_file(sha256)).await;
    Ok(())
}

/// Drop every pending upload of a season (season reset).
pub async fn clear_pending(db: &Db, season: Id) {
    let dirs = config::get().season(season);
    let _ = tokio::fs::remove_dir_all(&dirs.pending).await;
    let _ = tokio::fs::create_dir_all(&dirs.pending).await;
    let _ = db.clear_world_requests(season).await;
}

/// A checked player's slot name, or why it can't be one.
pub fn slot_name_of_player(p: &crate::checker::PlayerResult) -> Result<String, String> {
    if !p.plain_name {
        return Err(
            "That YAML needs one plain top-level name: as text, in quotes if it could read \
             as something else, and not one its triggers can change."
                .to_string(),
        );
    }
    slot_name_of(&p.name)
}

/// The longest game name a held signup goes by until its check reads the real one.
const MAX_GAME: usize = 80;

/// A YAML's `name:` as its slot's name, or why it can't be. Archipelago trims names to
/// 16 characters and fills in placeholders when it rolls the seed, and that's the name
/// the host's slot is found by, so the name has to be what it'll end up as.
pub fn slot_name_of(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.chars().any(char::is_control) {
        return Err("That name has control characters in it.".to_string());
    }
    if name.is_empty() {
        return Err("Couldn't find a top-level name: in that YAML.".to_string());
    }
    if name.contains(['{', '}'])
        || name.contains("%%")
        || name.contains("%number%")
        || name.contains("%player%")
    {
        return Err(format!(
            "The name '{name}' needs to be one plain name, without placeholders or weights: \
             it's how your slot is found once the seed is rolled."
        ));
    }
    if name.chars().count() > 16 {
        return Err(format!(
            "The name '{name}' is too long: Archipelago cuts names at 16 characters."
        ));
    }
    if name == "Archipelago" {
        return Err("Archipelago doesn't allow that name.".to_string());
    }
    Ok(name.to_string())
}

/// What a first look at a held YAML can tell without running its world: it's YAML,
/// for one player, with a plain name, and for the game waiting for approval
/// (`world_game`, empty if the manifest didn't say). Returns its slot name and game.
/// Options only the world's code understands wait for the check on approval.
pub fn judge_first_look(
    look: &crate::ap_adapter::FirstLook,
    world_game: &str,
) -> Result<(String, String), String> {
    if let Some(e) = &look.error {
        return Err(format!("That isn't valid YAML: {e}"));
    }
    if look.players.len() > 1 {
        return Err(ONE_GAME_PER_FILE.to_string());
    }
    let Some(player) = look.players.first() else {
        return Err("Couldn't read a player from that YAML.".to_string());
    };
    let name = player.name.as_deref().ok_or(
        "Couldn't find a top-level name: in that YAML. It has to be plain text, in quotes if \
         it could read as something else.",
    )?;
    let name = slot_name_of(name)?;
    if player.games.is_empty() {
        return Err("Couldn't find a game: in that YAML.".to_string());
    }
    if !world_game.is_empty() && !player.games.iter().any(|g| g == world_game) {
        return Err(format!(
            "That YAML is for {}, but the world waiting for approval is {world_game}.",
            player.games.join(", ")
        ));
    }
    let game = if world_game.is_empty() {
        player.games[0].chars().take(MAX_GAME).collect()
    } else {
        world_game.to_string()
    };
    Ok((name, game))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn apworld(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            for (name, body) in entries {
                z.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                z.write_all(body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn manifest_is_read_from_the_world_folder() {
        let w = apworld(&[
            ("foo/__init__.py", ""),
            (
                "foo/archipelago.json",
                r#"{"game": "Foo Quest", "world_version": "1.4.0", "version": 7}"#,
            ),
        ]);
        assert!(looks_like_apworld(&w));
        assert_eq!(
            manifest(&w),
            Manifest {
                game: "Foo Quest".into(),
                version: "1.4.0".into()
            }
        );
        assert_eq!(
            manifest(&apworld(&[("foo/__init__.py", "")])),
            Manifest::default()
        );
        assert_eq!(manifest(b"not a zip"), Manifest::default());
    }

    #[test]
    fn upload_names_are_slugged_and_reserved_names_refused() {
        assert_eq!(
            world_file("Pokemon Crystal.apworld").unwrap(),
            "Pokemon_Crystal.apworld"
        );
        assert!(world_file("tracker.apworld").is_err());
    }

    #[test]
    fn a_slot_name_is_what_archipelago_will_call_it() {
        assert_eq!(slot_name_of("  Sam "), Ok("Sam".to_string()));
        assert_eq!(
            slot_name_of("Sixteen letters!"),
            Ok("Sixteen letters!".to_string())
        );
        for bad in [
            "",
            "Player{number}",
            "Sam%player%",
            "50%%",
            "Ann\u{1f}",
            "{'A': 1, 'B': 1}",
            "seventeen letters",
            "Archipelago",
        ] {
            assert!(slot_name_of(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_first_look_is_judged() {
        use crate::ap_adapter::{FirstLook, LookedPlayer};
        let one = |name: Option<&str>, games: &[&str]| FirstLook {
            players: vec![LookedPlayer {
                name: name.map(str::to_string),
                games: games.iter().map(|g| g.to_string()).collect(),
            }],
            error: None,
        };
        let judge = |l: &FirstLook| judge_first_look(l, "Foo");
        assert_eq!(
            judge(&one(Some("Sam"), &["Foo"])),
            Ok(("Sam".into(), "Foo".into()))
        );
        assert_eq!(
            judge(&one(Some("Sam"), &["Bar", "Foo"])),
            Ok(("Sam".into(), "Foo".into()))
        );
        assert_eq!(
            judge_first_look(&one(Some("Sam"), &["Bar"]), ""),
            Ok(("Sam".into(), "Bar".into())),
            "a manifest that names no game can't be compared"
        );
        let long = "x".repeat(17);
        let cases = [
            (one(Some(long.as_str()), &["Foo"]), "too long"),
            (one(None, &["Foo"]), "top-level name"),
            (one(Some("Sam"), &[]), "game:"),
            (one(Some("Sam"), &["Bar"]), "is for Bar"),
        ];
        for (look, why) in cases {
            assert!(judge(&look).unwrap_err().contains(why), "{why}");
        }
        let mut two = one(Some("A"), &["Foo"]);
        two.players.push(LookedPlayer {
            name: Some("B".into()),
            games: vec!["Foo".into()],
        });
        assert!(judge(&two).unwrap_err().contains("several players"));
        let broken = FirstLook {
            players: vec![],
            error: Some("line 1".into()),
        };
        assert!(judge(&broken).unwrap_err().contains("valid YAML"));
    }
}

#[cfg(test)]
mod version_tests {
    use super::*;

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write;
        let mut out = std::io::Cursor::new(Vec::new());
        let mut z = zip::ZipWriter::new(&mut out);
        for (name, body) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
        out.into_inner()
    }

    #[test]
    fn the_manifest_is_the_one_archipelago_reads() {
        let m = |game: &str| format!(r#"{{"game": "{game}", "world_version": "1.2.3"}}"#);
        // The root one first, wherever else one is.
        let z = zip_of(&[
            ("w/archipelago.json", &m("Decoy")),
            ("archipelago.json", &m("Real")),
        ]);
        assert_eq!(manifest(&z).game, "Real");
        // Else the first one at any depth (AP: first entry ending in it).
        let z = zip_of(&[
            ("w/deep/archipelago.json", &m("Deep")),
            ("w/archipelago.json", &m("Later")),
        ]);
        assert_eq!(manifest(&z).game, "Deep");
        let z = zip_of(&[("w/archipelago.json", &m("Plain"))]);
        assert_eq!(
            (manifest(&z).game.as_str(), manifest(&z).version.as_str()),
            ("Plain", "1.2.3")
        );
        assert_eq!(manifest(b"not a zip"), Manifest::default());
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(version_newer("1.10.0", "1.9.2"));
        assert!(version_newer("v2", "1.99"));
        assert!(version_newer("1.0.1", "1.0"));
        assert!(!version_newer("1.0", "1.0.0"));
        assert!(!version_newer("1.0.0", "1.1"));
        assert!(!version_newer("", "1.0"), "no version is never newer");
        assert!(!version_newer("2.0-beta", "1.0"), "not a dotted version");
    }

    #[test]
    fn the_catalog_entry_for_a_game_is_its_newest() {
        let w = |game: &str, version: &str, sha: &str| CatalogWorld {
            sha256: sha.into(),
            file: format!("{sha}.apworld"),
            game: game.into(),
            version: version.into(),
            note: String::new(),
            public_note: String::new(),
            added_by: "root".into(),
            added_at: 0,
        };
        let cat = [
            w("Bar Finder", "1.0.0", "a"),
            w("Bar Finder", "1.2.0", "b"),
            w("bar finder", "9", "d"),
            w("Foo", "3", "c"),
        ];
        assert_eq!(
            catalog_for_game(&cat, "Bar Finder").map(|w| w.sha256.as_str()),
            Some("b"),
            "the newest, and names match exactly, as in Archipelago"
        );
        assert!(catalog_for_game(&cat, "").is_none());
        assert!(catalog_for_game(&cat, "Baz").is_none());
    }
}
