//! Regression test for:
//! Decompressing a valid PKWare DCL stream produced output whose first
//! ~4KiB was correct but which diverged afterwards. The fixture
//! `undhr.z` (and its expected plaintext `undhr.md`) comes from the
//! `explode` crate's examples:
//! https://github.com/agrif/explode/tree/master/src/examples

use pklib::{explode_bytes, CompressionMode, DictionarySize, ExplodeReader, ImplodeWriter};
use std::io::Read;
use std::io::Write;

const COMPRESSED: &[u8] = include_bytes!("fixtures/undhr.z");
const EXPECTED: &[u8] = include_bytes!("fixtures/undhr.md");

#[test]
fn diverging_4kib_explode_bytes_matches_reference_output() {
    let decompressed = explode_bytes(COMPRESSED).expect("decompression should succeed");

    if decompressed != EXPECTED {
        let div = first_divergence(&decompressed, EXPECTED);
        panic!(
            "decompressed output differs from reference: got {} bytes, expected {} bytes, \
             first divergence at byte {:?} (note: 0x1000 = 4096, the dictionary flush boundary)",
            decompressed.len(),
            EXPECTED.len(),
            div
        );
    }
}

#[test]
fn diverging_4kib_streaming_reader_matches_reference_output() {
    // Same check through the incremental Read interface, using small reads
    // so multiple expand() calls / dictionary flushes are exercised.
    let mut reader = ExplodeReader::new(std::io::Cursor::new(COMPRESSED)).unwrap();
    let mut decompressed = Vec::new();
    let mut chunk = [0u8; 337]; // deliberately odd size
    loop {
        match reader.read(&mut chunk).unwrap() {
            0 => break,
            n => decompressed.extend_from_slice(&chunk[..n]),
        }
    }

    assert_eq!(
        decompressed.len(),
        EXPECTED.len(),
        "length mismatch (first divergence at {:?})",
        first_divergence(&decompressed, EXPECTED)
    );

    assert_eq!(
        decompressed,
        EXPECTED,
        "content mismatch, first divergence at {:?}",
        first_divergence(&decompressed, EXPECTED)
    );
}

#[test]
fn diverging_4kib_round_trip_across_flush_boundary() {
    // Data with long repetitions ensures a match copy straddles the
    // 0x2000 output-buffer flush boundary, which is what corrupted the
    // sliding dictionary before the fix.
    let mut data = Vec::new();
    for i in 0..2000u32 {
        data.extend_from_slice(format!("line {i}: the quick brown fox jumps over the lazy dog\n").as_bytes());
    }

    let compressed = pklib::implode_bytes(
        &data,
        CompressionMode::Binary,
        DictionarySize::Size4K,
    )
    .expect("compression should succeed");

    let decompressed = explode_bytes(&compressed).expect("decompression should succeed");
    assert_eq!(
        decompressed,
        data,
        "round-trip mismatch: got {} bytes, expected {} bytes, first divergence at {:?}",
        decompressed.len(),
        data.len(),
        first_divergence(&decompressed, &data)
    );
}

// Regression test for finding silent data loss in ImplodeWriter.
#[test]
fn compressor_data_loss_when_work_buff_fills_exactly() {
    // batch1 + batch2 == WORK_BUFF_SIZE from src/implode/mod.rs exactly.
    //
    // After write(batch1): process_input copies 4608 bytes, work_bytes=4608,
    //   compresses them, compressed_pos=4608, input empty → break.
    //
    // After write(batch2): process_input copies 4100 bytes, work_bytes=8708,
    //   compresses them, compressed_pos=8708, input empty → break.
    //   The slide block (lines 117-123) is never reached because input IS empty.
    //
    // write(batch3): available_space = WORK_BUFF_SIZE - WORK_BUFF_SIZE = 0,
    //   copy_len = 0, work_bytes (8708) <= compressed_pos (8708) → break.
    //   batch3 stays in input_buffer and is never compressed.
    //
    // finish() → flush_remaining_data() → process_input(): same dead state, data lost.
    let batch1 = vec![0x11u8; 4608]; // fills part of work_buff
    let batch2 = vec![0x22u8; 4100]; // 4608 + 4100 = WORK_BUFF_SIZE: fills it exactly
    let batch3 = vec![0x33u8; 4096]; // silently lost

    let total = batch1.len() + batch2.len() + batch3.len();

    let mut compressed = Vec::new();

    {
        let mut w =
            ImplodeWriter::new(&mut compressed, CompressionMode::Binary, DictionarySize::Size4K)
                .expect("writer creation should succeed");
        w.write_all(&batch1).expect("write batch1 should succeed");
        w.write_all(&batch2).expect("write batch2 should succeed");
        // The following write returns Ok(4096) but batch3 was silently discarded.
        w.write_all(&batch3).expect("write batch3 returns Ok — data loss is silent");
        w.finish().expect("finish should succeed — batch3 was already silently lost");
    }

    let decompressed = pklib::explode_bytes(&compressed).expect("decompression should succeed");

    let mut expected = batch1;
    expected.extend_from_slice(&batch2);
    expected.extend_from_slice(&batch3);

    assert_eq!(
        decompressed.len(),
        total,
        "only {}/{} bytes survived compression; {} bytes of batch3 silently lost",
        decompressed.len(),
        total,
        total.saturating_sub(decompressed.len()),
    );

    assert_eq!(
        decompressed,
        expected,
        "decompressed content differs from original — batch3 data corrupted or missing"
    );
}

/// Report the first byte where two buffers differ (for a useful message).
fn first_divergence(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter().zip(b.iter()).position(|(x, y)| x != y)
}
