//! AT-SPI2 insertion: find the focused editable widget on the accessibility
//! bus and call `org.a11y.atspi.EditableText.InsertText` on it.
//!
//! Focus discovery uses the Collection interface: the registry root lists
//! running applications; each application's root implements Collection, which
//! we query with a match rule for `State::Focused`. This is a one-shot query
//! (no event tracking needed) and works the same on X11 and Wayland.
//!
//! Requirements on the user side: at-spi2 running (default on GNOME/KDE) and
//! toolkit accessibility enabled (GTK/Qt do this when the a11y bus is up;
//! Electron apps need --force-renderer-accessibility). We detect and report
//! via the doctor rather than failing silently.

use atspi::connection::AccessibilityConnection;
use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::collection::CollectionProxy;
use atspi::proxy::editable_text::EditableTextProxy;
use atspi::proxy::text::TextProxy;
use atspi::{Interface, MatchType, ObjectMatchRule, SortOrder, State, StateSet};
use laf_core::types::{EngineError, EngineResult};

const REGISTRY_DEST: &str = "org.a11y.atspi.Registry";
const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";
/// Don't scan absurd numbers of applications in one shot.
const MAX_APPS: usize = 48;

pub async fn insert(text: &str) -> EngineResult<()> {
    let (dest, path, conn) = find_focused_editable().await?;

    // Prefer replacing an active selection; otherwise insert at the caret.
    let text_proxy = TextProxy::builder(&conn)
        .destination(dest.as_str())
        .and_then(|b| b.path(path.as_str()))
        .map_err(zerr)?
        .build()
        .await
        .map_err(zerr)?;
    let editable = EditableTextProxy::builder(&conn)
        .destination(dest.as_str())
        .and_then(|b| b.path(path.as_str()))
        .map_err(zerr)?
        .build()
        .await
        .map_err(zerr)?;

    let mut insert_at = text_proxy.caret_offset().await.unwrap_or(0);
    if let Ok(n) = text_proxy.get_n_selections().await {
        if n > 0 {
            if let Ok((start, end)) = text_proxy.get_selection(0).await {
                if end > start {
                    let _ = editable.delete_text(start, end).await;
                    insert_at = start;
                }
            }
        }
    }

    let ok = editable
        .insert_text(insert_at, text, text.chars().count() as i32)
        .await
        .map_err(|e| EngineError::Insertion(format!("EditableText.InsertText: {e}")))?;
    if !ok {
        return Err(EngineError::Insertion("EditableText.InsertText returned false".into()));
    }
    Ok(())
}

/// Locate the focused accessible that implements EditableText.
/// Returns (bus name, object path, connection).
async fn find_focused_editable() -> EngineResult<(String, String, zbus::Connection)> {
    let a11y = AccessibilityConnection::new()
        .await
        .map_err(|e| EngineError::Insertion(format!("a11y bus unavailable: {e}")))?;
    let conn = a11y.connection().clone();

    let root = AccessibleProxy::builder(&conn)
        .destination(REGISTRY_DEST)
        .and_then(|b| b.path(ROOT_PATH))
        .map_err(zerr)?
        .build()
        .await
        .map_err(zerr)?;

    let apps = root
        .get_children()
        .await
        .map_err(|e| EngineError::Insertion(format!("registry children: {e}")))?;

    let rule = ObjectMatchRule::builder()
        .states(StateSet::new(State::Focused), MatchType::All)
        .build();

    for app in apps.into_iter().take(MAX_APPS) {
        // ObjectRefOwned::name() is None for the null object — skip those.
        let Some(app_dest) = app.name().map(|n| n.to_string()) else { continue };
        let app_path = app.path().to_string();
        let Ok(builder) = CollectionProxy::builder(&conn)
            .destination(app_dest.as_str())
            .and_then(|b| b.path(app_path.as_str()))
        else {
            continue;
        };
        let Ok(collection) = builder.build().await else { continue };
        let matches = match collection.get_matches(rule.clone(), SortOrder::Canonical, 1, false).await
        {
            Ok(m) => m,
            Err(_) => continue, // app without Collection support
        };
        let Some(hit) = matches.into_iter().next() else { continue };
        let Some(dest) = hit.name().map(|n| n.to_string()) else { continue };
        let path = hit.path().to_string();

        // Confirm it exposes EditableText before committing to it.
        let Ok(acc_builder) = AccessibleProxy::builder(&conn)
            .destination(dest.as_str())
            .and_then(|b| b.path(path.as_str()))
        else {
            continue;
        };
        let Ok(acc) = acc_builder.build().await else { continue };
        match acc.get_interfaces().await {
            Ok(ifaces) if ifaces.contains(Interface::EditableText) => {
                return Ok((dest, path, conn));
            }
            Ok(_) => {
                return Err(EngineError::Insertion(
                    "focused widget does not implement EditableText".into(),
                ));
            }
            Err(_) => continue,
        }
    }
    Err(EngineError::Insertion("no focused editable widget found on the a11y bus".into()))
}

fn zerr(e: zbus::Error) -> EngineError {
    EngineError::Insertion(format!("AT-SPI2 D-Bus: {e}"))
}
