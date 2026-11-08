import json
import re
from decimal import Decimal, InvalidOperation

KECCAK_ROUND_CONSTANTS = (
    0x0000000000000001, 0x0000000000008082, 0x800000000000808A, 0x8000000080008000, 0x000000000000808B, 0x0000000080000001,
    0x8000000080008081, 0x8000000000008009, 0x000000000000008A, 0x0000000000000088, 0x0000000080008009, 0x000000008000000A,
    0x000000008000808B, 0x800000000000008B, 0x8000000000008089, 0x8000000000008003, 0x8000000000008002, 0x8000000000000080,
    0x000000000000800A, 0x800000008000000A, 0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008)
KECCAK_ROTATIONS = (
    (0, 36, 3, 41, 18), (1, 44, 10, 45, 2), (62, 6, 43, 15, 61), (28, 55, 25, 21, 56), (27, 20, 39, 8, 14))
KECCAK_RATE_BYTES = 136
LANE_MASK = (1 << 64) - 1

CURVE_P = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEFFFFFC2F
CURVE_N = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141
CURVE_G = (0x79BE667EF9DCBBAC55A06295CE870B07029BFCDB2DCE28D959F2815B16F81798,
           0x483ADA7726A3C4655DA4FBFC0E1108A8FD17B448A68554199C47D08FFB10D4B8)

DOMAIN_TYPE = b'EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)'
AGENT_TYPE = b'Agent(string source,bytes32 connectionId)'
EXCHANGE_DOMAIN_NAME = b'Exchange'
EXCHANGE_DOMAIN_VERSION = b'1'
EXCHANGE_DOMAIN_CHAIN_ID = 1337
MAINNET_SOURCE = b'a'
MILLISECONDS_PER_SECOND = 1000
MAXIMUM_WIRE_DECIMAL_PLACES = 8
PLAIN_DECIMAL_FORMAT = re.compile(r'^(0|[1-9][0-9]*)(\.[0-9]+)?$')


def rotate_left(lane: int, shift: int) -> int:
    return ((lane << shift) | (lane >> (64 - shift))) & LANE_MASK if shift else lane


def keccak_permutation(lanes: list[list[int]]) -> None:
    for round_constant in KECCAK_ROUND_CONSTANTS:
        column_parity = [lanes[x][0] ^ lanes[x][1] ^ lanes[x][2] ^ lanes[x][3] ^ lanes[x][4] for x in range(5)]
        for x in range(5):
            mix = column_parity[(x - 1) % 5] ^ rotate_left(column_parity[(x + 1) % 5], 1)
            for y in range(5):
                lanes[x][y] ^= mix
        rotated = [[0] * 5 for _ in range(5)]
        for x in range(5):
            for y in range(5):
                rotated[y][(2 * x + 3 * y) % 5] = rotate_left(lanes[x][y], KECCAK_ROTATIONS[x][y])
        for x in range(5):
            for y in range(5):
                lanes[x][y] = rotated[x][y] ^ ((~rotated[(x + 1) % 5][y]) & rotated[(x + 2) % 5][y])
        lanes[0][0] ^= round_constant


def keccak256(data: bytes) -> bytes:
    padded = bytearray(data) + b'\x01'
    padded += b'\x00' * (-len(padded) % KECCAK_RATE_BYTES)
    padded[-1] |= 0x80
    lanes = [[0] * 5 for _ in range(5)]
    for offset in range(0, len(padded), KECCAK_RATE_BYTES):
        block = padded[offset:offset + KECCAK_RATE_BYTES]
        for index in range(KECCAK_RATE_BYTES // 8):
            lanes[index % 5][index // 5] ^= int.from_bytes(block[index * 8:index * 8 + 8], 'little')
        keccak_permutation(lanes)
    return b''.join(lanes[index % 5][index // 5].to_bytes(8, 'little') for index in range(4))


def msgpack_length_header(length: int, fix_base: int, fix_limit: int, code8: int | None, code16: int, code32: int) -> bytes:
    if length < fix_limit:
        return bytes([fix_base | length])
    if code8 is not None and length < 0x100:
        return bytes([code8, length])
    if length < 0x10000:
        return bytes([code16]) + length.to_bytes(2, 'big')
    return bytes([code32]) + length.to_bytes(4, 'big')


def msgpack_pack_unsigned(value: int) -> bytes:
    if value < 0x80:
        return bytes([value])
    for code, size in ((0xcc, 1), (0xcd, 2), (0xce, 4), (0xcf, 8)):
        if value < 1 << (8 * size):
            return bytes([code]) + value.to_bytes(size, 'big')
    raise OverflowError('integer above uint64')


def msgpack_pack(value) -> bytes:
    if value is True:
        return b'\xc3'
    if value is False:
        return b'\xc2'
    if isinstance(value, int):
        return msgpack_pack_unsigned(value)
    if isinstance(value, str):
        raw = value.encode()
        return msgpack_length_header(len(raw), 0xa0, 32, 0xd9, 0xda, 0xdb) + raw
    if isinstance(value, list):
        return msgpack_length_header(len(value), 0x90, 16, None, 0xdc, 0xdd) + b''.join(msgpack_pack(item) for item in value)
    if isinstance(value, dict):
        body = b''.join(msgpack_pack(key) + msgpack_pack(item) for key, item in value.items())
        return msgpack_length_header(len(value), 0x80, 16, None, 0xde, 0xdf) + body
    raise TypeError(f'cannot pack {type(value).__name__}')


def point_add(first: tuple | None, second: tuple | None) -> tuple | None:
    if first is None:
        return second
    if second is None:
        return first
    if first[0] == second[0] and (first[1] + second[1]) % CURVE_P == 0:
        return None
    if first == second:
        slope = 3 * first[0] * first[0] * pow(2 * first[1], -1, CURVE_P) % CURVE_P
    else:
        slope = (second[1] - first[1]) * pow(second[0] - first[0], -1, CURVE_P) % CURVE_P
    x = (slope * slope - first[0] - second[0]) % CURVE_P
    return x, (slope * (first[0] - x) - first[1]) % CURVE_P


def point_multiply(scalar: int, point: tuple | None) -> tuple | None:
    result, addend = None, point
    while scalar:
        if scalar & 1:
            result = point_add(result, addend)
        addend = point_add(addend, addend)
        scalar >>= 1
    return result


def recover_address(digest: bytes, r_hex: str, s_hex: str, v: int) -> str:
    r, s, recovery_id = int(r_hex, 16), int(s_hex, 16), v - 27
    x = r + (CURVE_N if recovery_id & 2 else 0)
    y = pow((pow(x, 3, CURVE_P) + 7) % CURVE_P, (CURVE_P + 1) // 4, CURVE_P)
    if y & 1 != recovery_id & 1:
        y = CURVE_P - y
    message_scalar = int.from_bytes(digest, 'big') % CURVE_N
    sum_point = point_add(point_multiply(s, (x, y)), point_multiply((CURVE_N - message_scalar) % CURVE_N, CURVE_G))
    recovered = point_multiply(pow(r, -1, CURVE_N), sum_point)
    return '0x' + keccak256(recovered[0].to_bytes(32, 'big') + recovered[1].to_bytes(32, 'big'))[-20:].hex()


def decimal_text_to_wire(text: str) -> str:
    if not isinstance(text, str) or not PLAIN_DECIMAL_FORMAT.match(text):
        raise ValueError(f'not a plain decimal string: {text}')
    try:
        if Decimal(text) <= 0:
            raise ValueError(f'not above zero: {text}')
    except InvalidOperation as parse_error:
        raise ValueError(f'not a decimal: {text}') from parse_error
    whole_part, _, fraction_part = text.partition('.')
    trimmed_fraction = fraction_part.rstrip('0')
    if len(trimmed_fraction) > MAXIMUM_WIRE_DECIMAL_PLACES:
        raise ValueError(f'more than {MAXIMUM_WIRE_DECIMAL_PLACES} decimals: {text}')
    return f'{whole_part}.{trimmed_fraction}' if trimmed_fraction else whole_part


def expected_action(intent: dict) -> dict:
    kind = intent['kind']
    if kind == 'order':
        return {
            'type': 'order',
            'orders': [{
                'a': intent['asset_index'],
                'b': intent['is_buy'],
                'p': decimal_text_to_wire(intent['limit_px']),
                's': decimal_text_to_wire(intent['size']),
                'r': intent['reduce_only'],
                't': {'limit': {'tif': intent['tif']}},
            }],
            'grouping': 'na',
        }
    if kind == 'cancel':
        return {'type': 'cancel', 'cancels': [{'a': intent['asset_index'], 'o': intent['oid']}]}
    if kind == 'schedule_cancel':
        return {'type': 'scheduleCancel', 'time': intent['time_ms']}
    raise ValueError(f'unknown kind {kind}')


def expires_after_milliseconds(intent: dict) -> int:
    return intent['expires_at'] * MILLISECONDS_PER_SECOND


def exchange_domain_separator() -> bytes:
    return keccak256(keccak256(DOMAIN_TYPE) + keccak256(EXCHANGE_DOMAIN_NAME) + keccak256(EXCHANGE_DOMAIN_VERSION)
                     + EXCHANGE_DOMAIN_CHAIN_ID.to_bytes(32, 'big') + bytes(32))


def action_digest(action: dict, venue_nonce: int, vault_address: str, expires_after: int) -> bytes:
    preimage = msgpack_pack(action) + venue_nonce.to_bytes(8, 'big')
    preimage += b'\x01' + bytes.fromhex(vault_address[2:]) if vault_address else b'\x00'
    preimage += b'\x00' + expires_after.to_bytes(8, 'big')
    agent_struct = keccak256(keccak256(AGENT_TYPE) + keccak256(MAINNET_SOURCE) + keccak256(preimage))
    return keccak256(b'\x19\x01' + exchange_domain_separator() + agent_struct)


def actions_are_identical(first: dict, second: dict) -> bool:
    return json.dumps(first) == json.dumps(second)
