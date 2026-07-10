pub(crate) fn decompress_size_prepended(
    input: &[u8],
    max_decoded_len: usize,
    context: &str,
) -> Result<Vec<u8>, String> {
    let (decoded_len, compressed) = lz4_flex::block::uncompressed_size(input)
        .map_err(|err| format!("read {context} LZ4 size: {err}"))?;
    if decoded_len > max_decoded_len {
        return Err(format!(
            "{context} decoded size {decoded_len} exceeds max {max_decoded_len}"
        ));
    }
    let mut decoded = Vec::new();
    decoded
        .try_reserve_exact(decoded_len)
        .map_err(|err| format!("allocate {context} ({decoded_len} bytes): {err}"))?;
    decoded.resize(decoded_len, 0);
    let actual = lz4_flex::block::decompress_into(compressed, &mut decoded)
        .map_err(|err| format!("decompress {context}: {err}"))?;
    if actual != decoded_len {
        return Err(format!(
            "{context} decoded size mismatch expected={decoded_len} actual={actual}"
        ));
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_decode_round_trips_and_rejects_oversized_prefix() {
        let encoded = lz4_flex::compress_prepend_size(b"catalog");
        assert_eq!(
            decompress_size_prepended(&encoded, 7, "fixture").expect("decode fixture"),
            b"catalog"
        );

        let mut oversized = encoded;
        oversized[..4].copy_from_slice(&8u32.to_le_bytes());
        let err = decompress_size_prepended(&oversized, 7, "fixture")
            .expect_err("oversized prefix should fail before allocation");
        assert!(err.contains("exceeds max"));
    }
}
