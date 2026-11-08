#!/bin/bash
# Usage: run_local_network.sh [--directory path] up [validator_count] | status | stop <n> | start <n> | down | wipe
set -euo pipefail

chain_directory="$(cd "$(dirname "$0")" && pwd)"
network_directory="$chain_directory/build/localnet"
if [ "${1:-}" = "--directory" ]; then
    network_directory="$2"
    shift 2
fi
action="${1:-status}"
lockboxd_binary="$chain_directory/build/lockboxd"
chain_id="lockbox-local-1"
first_port=46000
ports_per_node=10
client_home="$network_directory/client"
genesis_coins="1000000ulockbox"
validator_power=10000000

node_home() { echo "$network_directory/node$1"; }
peer_port() { echo $((first_port + $1 * ports_per_node + 0)); }
rpc_port() { echo $((first_port + $1 * ports_per_node + 1)); }
rest_port() { echo $((first_port + $1 * ports_per_node + 2)); }
grpc_port() { echo $((first_port + $1 * ports_per_node + 3)); }
profiler_port() { echo $((first_port + $1 * ports_per_node + 4)); }
node_count() { cat "$network_directory/validator_count"; }
pid_file() { echo "$network_directory/node$1.pid"; }

is_node_running() {
    [ -f "$(pid_file "$1")" ] && kill -0 "$(cat "$(pid_file "$1")")" 2>/dev/null
}

set_toml_value() {
    local file="$1" section="$2" key="$3" value="$4"
    sed -i "/^\[$section\]/,/^\[/ s|^$key = .*|$key = $value|" "$file"
}

set_top_level_toml_value() {
    local file="$1" key="$2" value="$3"
    sed -i "0,/^$key = /{s|^$key = .*|$key = $value|}" "$file"
}

create_client_key() {
    "$lockboxd_binary" keys add "$1" --home "$client_home" --keyring-backend test --output json >/dev/null 2>&1
}

client_key_address() {
    "$lockboxd_binary" keys show "$1" -a --home "$client_home" --keyring-backend test 2>/dev/null
}

create_network() {
    local validator_count="$1"
    [ -x "$lockboxd_binary" ] || { echo "build first: go build -o build/lockboxd ."; exit 1; }
    mkdir -p "$network_directory" "$client_home"
    echo "$validator_count" > "$network_directory/validator_count"

    for node in $(seq 1 "$validator_count"); do
        "$lockboxd_binary" init "node$node" --chain-id "$chain_id" --default-denom ulockbox --home "$(node_home "$node")" >/dev/null 2>&1
    done

    local genesis_home
    genesis_home="$(node_home 1)"
    for account_name in admin relay outsider signer-1 signer-2 signer-3; do
        create_client_key "$account_name"
        "$lockboxd_binary" genesis add-genesis-account "$(client_key_address "$account_name")" "$genesis_coins" --home "$genesis_home" >/dev/null 2>&1
    done
    for node in $(seq 1 "$validator_count"); do
        create_client_key "operator$node"
        "$lockboxd_binary" genesis add-genesis-account "$(client_key_address "operator$node")" "$genesis_coins" --home "$genesis_home" >/dev/null 2>&1
    done

    local genesis_file="$genesis_home/config/genesis.json"
    local validators_json="[]"
    for node in $(seq 1 "$validator_count"); do
        validators_json="$(jq -c \
            --argjson existing "$validators_json" \
            --arg moniker "node$node" \
            --argjson power "$validator_power" \
            '$existing + [{consensus_public_key: .pub_key.value, power: ($power | tostring), moniker: $moniker, founding: true}]' \
            "$(node_home "$node")/config/priv_validator_key.json")"
    done
    jq --arg admin "$(client_key_address admin)" --argjson validators "$validators_json" \
        '.app_state.validatorset = {admin: $admin, validators: $validators}' \
        "$genesis_file" > "$genesis_file.new"
    mv "$genesis_file.new" "$genesis_file"
    "$lockboxd_binary" genesis validate "$genesis_file" --home "$genesis_home" >/dev/null

    local peers=""
    for node in $(seq 1 "$validator_count"); do
        peers+="$("$lockboxd_binary" comet show-node-id --home "$(node_home "$node")" 2>/dev/null)@127.0.0.1:$(peer_port "$node"),"
    done
    for node in $(seq 1 "$validator_count"); do
        local home
        home="$(node_home "$node")"
        cp "$genesis_file" "$home/config/genesis.json" 2>/dev/null || true
        local cometbft_config="$home/config/config.toml"
        local application_config="$home/config/app.toml"
        set_toml_value "$cometbft_config" p2p laddr "\"tcp://127.0.0.1:$(peer_port "$node")\""
        set_toml_value "$cometbft_config" p2p persistent_peers "\"${peers%,}\""
        set_toml_value "$cometbft_config" p2p addr_book_strict false
        set_toml_value "$cometbft_config" p2p allow_duplicate_ip true
        set_toml_value "$cometbft_config" rpc laddr "\"tcp://127.0.0.1:$(rpc_port "$node")\""
        set_toml_value "$cometbft_config" consensus timeout_commit "\"1s\""
        set_toml_value "$cometbft_config" rpc pprof_laddr "\"127.0.0.1:$(profiler_port "$node")\""
        set_top_level_toml_value "$application_config" minimum-gas-prices "\"0ulockbox\""
        set_toml_value "$application_config" api enable true
        set_toml_value "$application_config" api address "\"tcp://127.0.0.1:$(rest_port "$node")\""
        set_toml_value "$application_config" grpc address "\"127.0.0.1:$(grpc_port "$node")\""
    done
}

start_node() {
    local node="$1"
    if is_node_running "$node"; then
        echo "node$node already running"
        return
    fi
    setsid "$lockboxd_binary" start --home "$(node_home "$node")" > "$network_directory/node$node.log" 2>&1 < /dev/null &
    echo $! > "$(pid_file "$node")"
    echo "node$node started, pid $!, rpc 127.0.0.1:$(rpc_port "$node"), rest 127.0.0.1:$(rest_port "$node")"
}

stop_node() {
    local node="$1"
    if ! is_node_running "$node"; then
        echo "node$node not running"
        rm -f "$(pid_file "$node")"
        return
    fi
    local process_id
    process_id="$(cat "$(pid_file "$node")")"
    kill "$process_id"
    for _ in $(seq 1 100); do
        kill -0 "$process_id" 2>/dev/null || break
        sleep 0.1
    done
    kill -9 "$process_id" 2>/dev/null || true
    rm -f "$(pid_file "$node")"
    echo "node$node stopped"
}

show_status() {
    [ -f "$network_directory/validator_count" ] || { echo "no network at $network_directory"; return; }
    for node in $(seq 1 "$(node_count)"); do
        if is_node_running "$node"; then
            local process_id resident_kilobytes status_json
            process_id="$(cat "$(pid_file "$node")")"
            resident_kilobytes="$(awk '/VmRSS/ {print $2}' "/proc/$process_id/status")"
            status_json="$(curl -s --max-time 2 "http://127.0.0.1:$(rpc_port "$node")/status" || true)"
            echo "node$node pid=$process_id rss_mb=$((resident_kilobytes / 1024)) height=$(jq -r '.result.sync_info.latest_block_height // "?"' <<<"$status_json") block_time=$(jq -r '.result.sync_info.latest_block_time // "?"' <<<"$status_json")"
        else
            echo "node$node stopped"
        fi
    done
}

case "$action" in
    up)
        if [ ! -f "$network_directory/validator_count" ]; then
            create_network "${2:-3}"
        fi
        for node in $(seq 1 "$(node_count)"); do start_node "$node"; done
        ;;
    status) show_status ;;
    stop) stop_node "$2" ;;
    start) start_node "$2" ;;
    down)
        for node in $(seq 1 "$(node_count)"); do stop_node "$node"; done
        ;;
    wipe)
        for node in $(seq 1 "$(node_count)"); do
            if is_node_running "$node"; then echo "node$node is running, run down first"; exit 1; fi
        done
        rm -rf "$network_directory"
        ;;
    *)
        echo "usage: $0 [--directory path] up [validator_count] | status | stop <n> | start <n> | down | wipe"
        exit 1
        ;;
esac
