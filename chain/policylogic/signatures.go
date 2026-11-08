package policylogic

import (
	"crypto/ed25519"
	"encoding/hex"
	"errors"
	"fmt"
	"strings"

	secp256k1ecdsa "github.com/decred/dcrd/dcrec/secp256k1/v4/ecdsa"
	"golang.org/x/crypto/sha3"
)

func keccak256(data ...[]byte) []byte {
	hasher := sha3.NewLegacyKeccak256()
	for _, chunk := range data {
		hasher.Write(chunk)
	}
	return hasher.Sum(nil)
}

func eip191PersonalMessageHash(text string) []byte {
	prefix := fmt.Sprintf("\x19Ethereum Signed Message:\n%d", len(text))
	return keccak256([]byte(prefix), []byte(text))
}

func ChecksummedAddress(lowercaseAddress string) (string, error) {
	if err := ValidateEthereumAddress("address", lowercaseAddress); err != nil {
		return "", err
	}
	hexDigits := lowercaseAddress[2:]
	addressHash := hex.EncodeToString(keccak256([]byte(hexDigits)))
	var checksummed strings.Builder
	checksummed.WriteString("0x")
	for position, digit := range hexDigits {
		if digit >= 'a' && addressHash[position] >= '8' {
			checksummed.WriteString(strings.ToUpper(string(digit)))
		} else {
			checksummed.WriteRune(digit)
		}
	}
	return checksummed.String(), nil
}

func RecoverEip191Address(text string, signatureHex string) (string, error) {
	if len(signatureHex) != ownerSignatureHexLength || !lowercaseHexFormat.MatchString(signatureHex) {
		return "", errors.New("owner signature must be 130 lowercase hex characters")
	}
	signature, err := hex.DecodeString(signatureHex)
	if err != nil {
		return "", err
	}
	recoveryValue := signature[64]
	if recoveryValue >= 27 {
		recoveryValue -= 27
	}
	if recoveryValue > 1 {
		return "", errors.New("owner signature recovery value must be 27, 28, 0 or 1")
	}
	compactSignature := make([]byte, 65)
	compactSignature[0] = 27 + recoveryValue
	copy(compactSignature[1:], signature[:64])
	publicKey, _, err := secp256k1ecdsa.RecoverCompact(compactSignature, eip191PersonalMessageHash(text))
	if err != nil {
		return "", err
	}
	addressBytes := keccak256(publicKey.SerializeUncompressed()[1:])[12:]
	return "0x" + hex.EncodeToString(addressBytes), nil
}

func VerifyEip191Owner(text string, signatureHex string, ownerAddress string) error {
	recoveredAddress, err := RecoverEip191Address(text, signatureHex)
	if err != nil {
		return err
	}
	if recoveredAddress != ownerAddress {
		return fmt.Errorf("signed by %s, not by the owner %s", recoveredAddress, ownerAddress)
	}
	return nil
}

func VerifyEd25519Signature(publicKeyHex string, message []byte, signatureHex string) error {
	if err := ValidatePublicKeyHex("public key", publicKeyHex); err != nil {
		return err
	}
	publicKey, err := hex.DecodeString(publicKeyHex)
	if err != nil {
		return err
	}
	signature, err := hex.DecodeString(signatureHex)
	if err != nil || len(signature) != ed25519.SignatureSize {
		return errors.New("ed25519 signature must be 128 hex characters")
	}
	if !ed25519.Verify(publicKey, message, signature) {
		return errors.New("ed25519 signature does not verify")
	}
	return nil
}
