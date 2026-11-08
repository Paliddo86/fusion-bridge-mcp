The local vendor symlink points to ../rust-native-migration-r21/vendor, reusing
already frozen dependencies. Cargo.lock pins this tool's small dependency set:
serde, serde_json with preserve_order/float_roundtrip, and sha2 plus transitive
Rust crates. No dependency was downloaded. For a standalone review checkout,
copy that vendor directory or deliberately configure an ordinary Cargo registry;
the source does not inspect or mutate any package manager configuration.
