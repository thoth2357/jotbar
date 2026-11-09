use jotbar_core::remote::{Remote, Target};
use jotbar_core::{sync::sync, Store};
use std::sync::Mutex;

#[tokio::test]
async fn two_devices_via_shared_folder() {
    let shared = tempfile::tempdir().unwrap();
    let target = Target::Folder { path: shared.path().to_string_lossy().into() };
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = Mutex::new(Store::open(da.path()).unwrap());
    let b = Mutex::new(Store::open(db.path()).unwrap());
    let mut ra = Remote::connect(&target).unwrap();
    let mut rb = Remote::connect(&target).unwrap();

    let id = a.lock().unwrap().create("todo\n- ship it").unwrap().id;
    sync(&a, &mut ra).await.unwrap();
    // The shared folder holds a readable markdown file.
    let raw = std::fs::read_to_string(shared.path().join(format!("{id}.md"))).unwrap();
    assert!(raw.ends_with("todo\n- ship it"));

    sync(&b, &mut rb).await.unwrap();
    assert_eq!(b.lock().unwrap().read(&id).unwrap(), "todo\n- ship it");

    a.lock().unwrap().write(&id, "from A").unwrap();
    b.lock().unwrap().write(&id, "from B").unwrap();
    sync(&a, &mut ra).await.unwrap();
    let r = sync(&b, &mut rb).await.unwrap();
    assert_eq!(r.conflicts, 1);
    let s = b.lock().unwrap();
    let all: Vec<String> = s.list().iter().map(|m| s.read(&m.id).unwrap()).collect();
    assert!(all.contains(&"from A".to_string()));
    assert!(all.iter().any(|c| c.ends_with("from B")));
}
