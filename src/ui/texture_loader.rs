//! Decoding of images into textures off the main thread, with a cache.
//!
//! Decoding even a small JPEG takes 20 ms and more on a phone, and the same
//! images (avatars above all) are shown again and again while scrolling, as
//! list rows are recycled. Doing that on the main thread makes scrolling
//! stutter, so files are decoded in the blocking thread pool of GIO, and the
//! resulting textures are kept in a small least-recently-used cache, which
//! also lets widgets show an image they have shown before without waiting.
//!
//! Minithumbnails, the tiny previews that TDLib embeds in messages, are cheap
//! to decode, so they are decoded synchronously, but cached as well.

use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::hash::Hash;

use gtk::gdk;
use gtk::gio;
use gtk::glib;

use crate::utils;

/// The number of decoded files that are kept in the cache.
const FILE_CAPACITY: usize = 256;
/// The number of decoded minithumbnails that are kept in the cache.
const MINITHUMBNAIL_CAPACITY: usize = 256;

type LoadResult = Result<gdk::Texture, glib::Error>;

/// A map that keeps at most `capacity` entries, dropping the least recently
/// used one when full.
struct Lru<K, V> {
    capacity: usize,
    map: HashMap<K, V>,
    /// The keys from the least to the most recently used.
    order: VecDeque<K>,
}

impl<K: Hash + Eq + Clone, V: Clone> Lru<K, V> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn touch(&mut self, key: &K) {
        if let Some(index) = self.order.iter().position(|k| k == key) {
            let key = self.order.remove(index).unwrap();
            self.order.push_back(key);
        }
    }

    fn get(&mut self, key: &K) -> Option<V> {
        let value = self.map.get(key).cloned()?;
        self.touch(key);
        Some(value)
    }

    fn insert(&mut self, key: K, value: V) {
        if self.map.insert(key.clone(), value).is_some() {
            self.touch(&key);
            return;
        }
        self.order.push_back(key);
        while self.order.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            }
        }
    }
}

struct State {
    files: Lru<String, gdk::Texture>,
    minithumbnails: Lru<String, gdk::Texture>,
    /// The callers waiting for the files that are being decoded.
    pending: HashMap<String, Vec<async_channel::Sender<LoadResult>>>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        files: Lru::new(FILE_CAPACITY),
        minithumbnails: Lru::new(MINITHUMBNAIL_CAPACITY),
        pending: HashMap::new(),
    });
}

/// Returns the texture of the image file at `path` if it is in the cache.
///
/// This lets a widget show an image it has shown before right away, without
/// a frame without it.
pub(crate) fn cached(path: &str) -> Option<gdk::Texture> {
    STATE.with(|state| state.borrow_mut().files.get(&path.to_owned()))
}

/// Decodes the image file at `path` into a texture, off the main thread.
///
/// The texture is taken from the cache if possible. Concurrent loads of the
/// same file share a single decoding, which keeps going even when the
/// caller stops waiting for it.
pub(crate) async fn load(path: &str) -> LoadResult {
    if let Some(texture) = cached(path) {
        return Ok(texture);
    }

    let (sender, receiver) = async_channel::bounded(1);
    let is_first = STATE.with(
        |state| match state.borrow_mut().pending.entry(path.to_owned()) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().push(sender);
                false
            }
            Entry::Vacant(entry) => {
                entry.insert(vec![sender]);
                true
            }
        },
    );

    if is_first {
        let path = path.to_owned();
        glib::MainContext::ref_thread_default().spawn_local(async move {
            let result = decode(&path).await;

            let waiters = STATE.with(|state| {
                let mut state = state.borrow_mut();
                if let Ok(texture) = &result {
                    state.files.insert(path.clone(), texture.clone());
                }
                state.pending.remove(&path).unwrap_or_default()
            });
            for waiter in waiters {
                _ = waiter.try_send(result.clone());
            }
        });
    }

    receiver.recv().await.unwrap_or_else(|_| {
        Err(glib::Error::new(
            gio::IOErrorEnum::Cancelled,
            "The image decoding was dropped",
        ))
    })
}

/// Decodes the image file at `path` into a texture, off the main thread,
/// bypassing the cache.
///
/// This is meant for large images that are shown once, such as a photo that
/// is about to be sent, which would take a lot of memory in the cache.
pub(crate) async fn decode(path: &str) -> LoadResult {
    let path = path.to_owned();
    gio::spawn_blocking(move || gdk::Texture::from_filename(path))
        .await
        .unwrap_or_else(|_| {
            Err(glib::Error::new(
                gio::IOErrorEnum::Failed,
                "The image decoder panicked",
            ))
        })
}

/// Returns the texture of the given minithumbnail, or `None` if it could
/// not be decoded.
pub(crate) fn minithumbnail(minithumbnail: &tdlib::types::Minithumbnail) -> Option<gdk::Texture> {
    if let Some(texture) =
        STATE.with(|state| state.borrow_mut().minithumbnails.get(&minithumbnail.data))
    {
        return Some(texture);
    }

    let texture = utils::texture_from_minithumbnail(minithumbnail)?;
    STATE.with(|state| {
        state
            .borrow_mut()
            .minithumbnails
            .insert(minithumbnail.data.clone(), texture.clone())
    });
    Some(texture)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_drops_the_least_recently_used() {
        let mut lru = Lru::new(2);
        lru.insert(1, "a");
        lru.insert(2, "b");
        assert_eq!(lru.get(&1), Some("a"));
        lru.insert(3, "c");
        assert_eq!(lru.get(&2), None);
        assert_eq!(lru.get(&1), Some("a"));
        assert_eq!(lru.get(&3), Some("c"));
        lru.insert(3, "d");
        assert_eq!(lru.map.len(), 2);
        assert_eq!(lru.order.len(), 2);
        assert_eq!(lru.get(&3), Some("d"));
    }
}

/// Checks of the loader that need GTK, see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_check() {
    use gtk::prelude::*;

    use crate::model::test_support::isolated;

    eprintln!("images are decoded off the main thread and cached");

    isolated(|| {
        let dir = std::env::temp_dir().join(format!("paper-plane-texture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("image.png");
        let png = gdk::MemoryTexture::new(
            2,
            3,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(vec![255u8; 2 * 3 * 4]),
            2 * 4,
        )
        .save_to_png_bytes();
        std::fs::write(&path, &png).unwrap();
        let path = path.to_str().unwrap().to_owned();

        assert!(cached(&path).is_none());

        // Two concurrent loads share one decoding.
        let context = glib::MainContext::ref_thread_default();
        let (first, second) =
            context.block_on(async { futures::future::join(load(&path), load(&path)).await });
        let first = first.expect("a decoded image");
        let second = second.expect("a decoded image");
        assert_eq!((first.width(), first.height()), (2, 3));
        assert_eq!(first, second);
        assert!(STATE.with(|state| state.borrow().pending.is_empty()));

        // The texture is cached now.
        assert_eq!(cached(&path).as_ref(), Some(&first));

        // A missing file is an error, and is not cached.
        let missing = dir.join("missing.png");
        let missing = missing.to_str().unwrap();
        assert!(context.block_on(load(missing)).is_err());
        assert!(cached(missing).is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    });
}
