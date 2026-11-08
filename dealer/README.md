# lockbox-key-import

Splits an existing secp256k1 key (an agent key the venue already approved) into t-of-n `cggmp21` key shares and seals each share to one signer. Runs once, on the machine that already holds the key, never on a signer VM. Contract: `../docs/INTERFACES.md`, section Imported keys.

```
lockbox-key-import-dealer --key-ref kr-milestone-a-3 --expected-address 0x... --threshold 2 \
  --recipient s1=lockbox-1.pem --recipient s2=lockbox-2.pem --recipient s3=lockbox-3.pem --output-directory DIR  < key on stdin
```

The key is read from stdin (hidden when it is a terminal), never from an argument or a file it keeps. It refuses before splitting if the key's address is not `--expected-address`, and refuses a non-empty output directory. Output: `public_data.json` and `share-<signer>.json` (mode 0600), and one JSON line with the address.

| File | What |
|---|---|
| `src/imported_key_dealer.rs` | Parse and zero the key, check the address, Paillier primes in parallel, trusted dealer, seal, write. Only with the `dealer` feature |
| `src/imported_share_sealing.rs` | ECIES to a certificate's P-256 key: ECDH, HKDF-SHA256, AES-256-GCM |
| `src/imported_share_public_data.rs` | Public data, sealed share file, associated data, and the checks a signer runs after opening |
| `src/bin/lockbox-key-import-dealer.rs` | The CLI |

The signer depends on this crate with default features off, so `spof` (trusted dealer, key reconstruction) never compiles into `lockbox-signer`.

Zeroing: the key text and bytes are wiped; the library's secret scalars wipe themselves; each share is serialized into one pre-sized wiped buffer before sealing. Not wiped: the Paillier primes inside the shares (GMP integers, not the venue key) and any copy the kernel or the shell holds of stdin.
