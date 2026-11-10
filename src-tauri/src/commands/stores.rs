use tauri::State;

use crate::error::AppResult;
use crate::sources::stores::{BuyLink, BuyQuery};
use crate::sources::Sources;

/// Store pages selling the track or its album. Only pages that exist are
/// returned; a store that can't be reached is left out.
#[tauri::command]
pub async fn find_buy_links(sources: State<'_, Sources>, query: BuyQuery) -> AppResult<Vec<BuyLink>> {
    Ok(sources.stores.find(&sources.musicbrainz, &query).await)
}
