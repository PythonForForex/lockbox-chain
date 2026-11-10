#!/bin/bash
set -euo pipefail
chain_directory="$(cd "$(dirname "$0")" && pwd)"
toolchain_gopath="$(go env GOPATH)"
export PATH="$toolchain_gopath/bin:$PATH"
software_development_kit_proto="$toolchain_gopath/pkg/mod/github.com/cosmos/cosmos-sdk@v0.55.0/proto"
workspace="$chain_directory/build/proto_workspace"

rm -rf "$workspace"
mkdir -p "$workspace/lockbox/v1" "$workspace/lockbox/validatorset/v1" "$workspace/cosmos/msg/v1"
cp "$chain_directory/proto/lockbox.proto" "$workspace/lockbox/v1/"
cp "$chain_directory/proto/validatorset.proto" "$workspace/lockbox/validatorset/v1/"
cp "$software_development_kit_proto/cosmos/msg/v1/msg.proto" "$workspace/cosmos/msg/v1/"
chmod -R u+w "$workspace"
cat > "$workspace/buf.gen.yaml" <<'YAML'
version: v2
plugins:
  - local: protoc-gen-gocosmos
    out: generated
    opt:
      - plugins=grpc
      - paths=source_relative
YAML
cd "$workspace"
buf generate --path lockbox/v1/lockbox.proto --path lockbox/validatorset/v1/validatorset.proto
cp generated/lockbox/v1/lockbox.pb.go "$chain_directory/lockbox/lockbox.pb.go"
mkdir -p "$chain_directory/validatorset"
cp generated/lockbox/validatorset/v1/validatorset.pb.go "$chain_directory/validatorset/validatorset.pb.go"
