//! Reading index files maps the entries after the 4 KiB header. This must also work where the
//! page size is larger than the header (e.g. 16 KiB on Apple Silicon).

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use pbs_datastore::dynamic_index::DynamicIndexReader;
use pbs_datastore::file_formats::{DYNAMIC_SIZED_CHUNK_INDEX_1_0, FIXED_SIZED_CHUNK_INDEX_1_0};
use pbs_datastore::fixed_index::FixedIndexReader;
use pbs_datastore::index::IndexFile;

const HEADER_SIZE: usize = 4096;

fn write_index(name: &str, header: &[u8], entries: &[u8]) -> File {
    let path: PathBuf =
        std::env::temp_dir().join(format!("pbs-index-mmap-{}-{}", std::process::id(), name));
    let mut file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    file.write_all(header).unwrap();
    file.write_all(entries).unwrap();
    file
}

fn digest(i: usize) -> [u8; 32] {
    [i as u8; 32]
}

#[test]
fn test_dynamic_index_reader() {
    let count = 1000;
    let mut header = vec![0u8; HEADER_SIZE];
    header[..8].copy_from_slice(&DYNAMIC_SIZED_CHUNK_INDEX_1_0);

    let mut entries = Vec::new();
    for i in 0..count {
        entries.extend_from_slice(&(((i + 1) * 4096) as u64).to_le_bytes());
        entries.extend_from_slice(&digest(i));
    }

    let index = DynamicIndexReader::new(write_index("didx", &header, &entries)).unwrap();
    assert_eq!(index.index_count(), count);
    for i in 0..count {
        assert_eq!(index.chunk_end(i), ((i + 1) * 4096) as u64);
        assert_eq!(index.index_digest(i), Some(&digest(i)));
    }
}

#[test]
fn test_fixed_index_reader() {
    let count = 1000;
    let chunk_size = 4096u64;
    let mut header = vec![0u8; HEADER_SIZE];
    header[..8].copy_from_slice(&FIXED_SIZED_CHUNK_INDEX_1_0);
    header[64..72].copy_from_slice(&(count as u64 * chunk_size).to_le_bytes());
    header[72..80].copy_from_slice(&chunk_size.to_le_bytes());

    let entries: Vec<u8> = (0..count).flat_map(digest).collect();

    let index = FixedIndexReader::new(write_index("fidx", &header, &entries)).unwrap();
    assert_eq!(index.index_count(), count);
    for i in 0..count {
        assert_eq!(index.index_digest(i), Some(&digest(i)));
    }
}
