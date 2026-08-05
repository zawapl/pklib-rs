//! Hash table implementation for PKLib compression
//!
//! This module implements the hash table system used by PKLib for fast
//! pattern matching during compression. It ports the SortBuffer algorithm
//! from the original PKLib implementation.

use super::{byte_pair_hash, state::ImplodeState, HASH_TABLE_SIZE};

impl ImplodeState {
    /// Build hash table for the current work buffer
    /// This is a port of the SortBuffer function from PKLib implode.c
    pub fn sort_buffer(&mut self, buffer_begin: usize, buffer_end: usize) {
        // Ensure we have at least 2 bytes for pair hash
        if buffer_end <= buffer_begin + 1 {
            self.pair_count = 0;
            return;
        }

        self.pair_count = buffer_end - buffer_begin - 1;

        // Step 1: Zero the hash-to-index table
        self.phash_to_index.fill(0);

        // Step 2: Count occurrences of each PAIR_HASH in the input buffer
        // The table will contain the number of occurrences of each hash value
        for pos in buffer_begin..buffer_end - 1 {
            if pos + 1 < self.work_buff.len() {
                let hash = byte_pair_hash(&self.work_buff[pos..pos + 2]);
                if hash < HASH_TABLE_SIZE {
                    self.phash_to_index[hash] = self.phash_to_index[hash].saturating_add(1);
                }
            }
        }

        // Step 3: Convert the table to cumulative counts
        // Each element contains count of PAIR_HASHes that is less than or equal to element index
        let mut total_sum = 0u16;
        for hash_count in &mut self.phash_to_index {
            total_sum = total_sum.saturating_add(*hash_count);
            *hash_count = total_sum;
        }

        // Step 4: Build the offset table by processing buffer in reverse
        // This creates a table where each PAIR_HASH points to its first occurrence
        for pos in (buffer_begin..buffer_end - 1).rev() {
            if pos + 1 < self.work_buff.len() {
                let hash = byte_pair_hash(&self.work_buff[pos..pos + 2]);
                if hash < HASH_TABLE_SIZE {
                    // Decrement the count to get the index
                    self.phash_to_index[hash] = self.phash_to_index[hash].saturating_sub(1);

                    let index = self.phash_to_index[hash] as usize;
                    if index < self.phash_offs.len() {
                        // Store the relative offset from work_buff start
                        self.phash_offs[index] = (pos - buffer_begin) as u16;
                    }
                }
            }
        }
    }

    /// Find all positions where a specific byte pair hash occurs
    pub fn find_hash_positions(&self, hash: usize, current_pos: usize) -> Vec<usize> {
        let mut positions = Vec::new();

        if hash >= HASH_TABLE_SIZE {
            return positions;
        }

        let (start, end) = self.hash_bucket_range(hash);
        let min_offset = current_pos.saturating_sub(self.dsize_bytes as usize);

        // Offsets within a bucket are stored in ascending order of position.
        for &raw_offset in &self.phash_offs[start..end] {
            let offset = raw_offset as usize;
            if offset >= min_offset && offset < current_pos {
                positions.push(offset);
            } else if offset >= current_pos {
                // Offsets are sorted, so we can stop here
                break;
            }
        }

        positions
    }

    /// Get the `[start, end)` range in `phash_offs` holding the occurrences of `hash`,
    /// sorted in ascending order of position.
    ///
    /// After `sort_buffer` runs, `phash_to_index[hash]` is the start index of that hash's
    /// bucket, and since buckets are laid out contiguously in cumulative-count order, the
    /// bucket's end is simply the start of the next bucket (`phash_to_index[hash + 1]`), or
    /// `pair_count` for the last bucket. Offset `0` is a legitimate position (the start of the
    /// sliding window) and must not be treated as a "no data" sentinel, which is why this
    /// (unlike the old implementation) never inspects the offset values themselves to decide
    /// whether a bucket is valid.
    fn hash_bucket_range(&self, hash: usize) -> (usize, usize) {
        let start = self.phash_to_index[hash] as usize;
        let end = if hash + 1 < HASH_TABLE_SIZE {
            self.phash_to_index[hash + 1] as usize
        } else {
            self.pair_count
        };
        (start, end)
    }

    /// Update hash table incrementally for a new byte pair
    /// This is used when sliding the compression window
    pub fn update_hash_incremental(&mut self, pos: usize) {
        if pos + 1 < self.work_buff.len() {
            let hash = byte_pair_hash(&self.work_buff[pos..pos + 2]);
            if hash < HASH_TABLE_SIZE {
                // Find next available slot in the hash offset table
                // This is a simplified incremental update - full PKLib does more complex management
                for i in 0..self.phash_offs.len() {
                    if self.phash_offs[i] == 0 {
                        self.phash_offs[i] = pos as u16;
                        break;
                    }
                }
            }
        }
    }

    /// Validate hash table consistency (for debugging)
    #[cfg(debug_assertions)]
    pub fn validate_hash_table(&self, buffer_start: usize, buffer_end: usize) -> bool {
        // Check that hash indices are within bounds
        for &index in &self.phash_to_index {
            if index as usize >= self.phash_offs.len() {
                return false;
            }
        }

        // Check that offsets point to valid positions
        for &offset in &self.phash_offs {
            if offset != 0 {
                let pos = offset as usize;
                if pos < buffer_start || pos >= buffer_end {
                    return false;
                }
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompressionMode, DictionarySize};

    #[test]
    fn test_byte_pair_hash() {
        let buffer = b"AB";
        let hash = byte_pair_hash(buffer);
        let expected = (b'A' as usize * 4) + (b'B' as usize * 5);
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_sort_buffer_basic() {
        let mut state = ImplodeState::new(CompressionMode::Binary, DictionarySize::Size1K).unwrap();

        // Set up test data
        let test_data = b"ABCABC";
        let len = test_data.len().min(state.work_buff.len());
        state.work_buff[..len].copy_from_slice(&test_data[..len]);

        // Sort the buffer
        state.sort_buffer(0, len);

        let hash_ab = byte_pair_hash(b"AB");
        let positions = state.find_hash_positions(hash_ab, len);
        assert!(!positions.is_empty());
    }

    #[test]
    fn test_hash_table_edge_cases() {
        let mut state = ImplodeState::new(CompressionMode::Binary, DictionarySize::Size1K).unwrap();

        // Test with empty buffer
        state.sort_buffer(0, 0);

        // Test with single byte
        state.work_buff[0] = b'A';
        state.sort_buffer(0, 1);

        // Test with two bytes
        state.work_buff[0] = b'A';
        state.work_buff[1] = b'B';
        state.sort_buffer(0, 2);

        let hash = byte_pair_hash(b"AB");
        let positions = state.find_hash_positions(hash, 2);
        assert_eq!(positions, vec![0]);
    }
}
