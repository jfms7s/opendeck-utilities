pub mod icons;
pub mod level;
pub mod profile;
pub mod text;
pub mod tile;

use level::{LevelView, strip_feedback, tile_image};
use openaction::{Instance, OpenActionResult};

pub const KEYPAD: &str = "Keypad";

/// Keys get a generated image (with the native title cleared so OpenDeck
/// doesn't paint a second copy); dials get touch-strip feedback.
pub async fn show_level(instance: &Instance, view: &LevelView) -> OpenActionResult<()> {
    if instance.controller == KEYPAD {
        instance.set_title(Some(String::new()), None).await?;
        instance.set_image(Some(tile_image(view)), None).await
    } else {
        instance.set_feedback(&strip_feedback(view)).await
    }
}

pub async fn show_profile(
    instance: &Instance,
    view: &crate::profile::ProfileView,
) -> OpenActionResult<()> {
    if instance.controller == KEYPAD {
        instance.set_title(Some(String::new()), None).await?;
        instance
            .set_image(Some(profile::tile_image(view)), None)
            .await
    } else {
        instance.set_feedback(&profile::strip_feedback(view)).await
    }
}
