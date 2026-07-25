//! Round-trips a real path list through both sides of the crate.
//! `PATH_LIST_FILE=<newline-separated paths> cargo test -p pathlist -- --ignored --nocapture`

use std::collections::BTreeMap;

#[test]
#[ignore]
fn round_trips_a_real_path_list() {
    let text = std::fs::read_to_string(std::env::var("PATH_LIST_FILE").unwrap()).unwrap();
    let mut by_dir: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        let (dir, name) = match line.rfind('/') {
            Some(i) => (&line[..i], &line[i + 1..]),
            None => ("", line),
        };
        by_dir.entry(dir).or_default().push(name);
    }
    let entries: Vec<(&str, Vec<&str>)> = by_dir
        .into_iter()
        .map(|(dir, mut names)| {
            names.sort_unstable();
            names.dedup();
            (dir, names)
        })
        .collect();
    let total: usize = entries.iter().map(|(_, n)| n.len()).sum();

    let body = pathlist::encode(&entries, 0xfeed);
    let frame = pathlist::compress(&body).unwrap();
    assert_eq!(pathlist::decompress(&frame).unwrap(), body);
    println!(
        "{total} paths / {} dirs -> {} body, {} compressed",
        entries.len(),
        body.len(),
        frame.len()
    );

    let list = pathlist::PathList::decode(&body).unwrap();
    assert_eq!(list.list_id(), 0xfeed);
    assert_eq!(list.dirs().len(), entries.len());
    let mut seen = 0;
    for (i, (dir, want)) in entries.iter().enumerate() {
        assert_eq!(&*list.dirs()[i], *dir);
        let got = list.names(i).unwrap();
        assert_eq!(got.len(), want.len(), "{dir}");
        for (a, b) in got.iter().zip(want) {
            assert_eq!(a, b, "{dir}");
        }
        seen += got.len();
    }
    assert_eq!(seen, total);
    println!(
        "all {total} paths round-tripped exactly; {:.2} MiB resident",
        list.resident_bytes() as f64 / (1024.0 * 1024.0)
    );

    // A presence map at real scale, which is the only thing that exercises a multi-byte `count`
    // varint and a bitmap whose length the decoder has to derive rather than be told.
    let present: Vec<bool> = (0..list.len()).map(|i| i % 3 != 0).collect();
    let unnamed = [pathlist::Unnamed {
        repository: 0,
        category: 3,
        hash: 0x1234_5678_9abc,
        split: true,
    }];
    let encoded = pathlist::encode_presence(&present, &unnamed, 0xfeed);
    let map = pathlist::Presence::decode(&encoded).unwrap();
    assert_eq!(map.list_id(), list.list_id());
    assert_eq!(map.len(), present.len());
    assert_eq!(map.unnamed(), unnamed);
    for (i, want) in present.iter().enumerate() {
        assert_eq!(map.contains(i), *want, "bit {i}");
    }
    assert!(!map.contains(present.len()));
    println!(
        "presence over all {} paths round-tripped exactly; {} bytes encoded",
        present.len(),
        encoded.len()
    );
}
