package policylogic

import (
	"errors"
	"fmt"
	"math/big"
	"regexp"
)

const (
	DecisionSigned          = "signed"
	DecisionRefused         = "refused"
	maximumReasonLength     = 512
	minimumCommitteeQuorum  = 2
	lowercaseHexKeyLength   = 64
	lowercaseHexHashLength  = 64
	ownerSignatureHexLength = 130
)

var (
	identifierFormat      = regexp.MustCompile(`^[A-Za-z0-9._:-]{1,128}$`)
	assetNameFormat       = regexp.MustCompile(`^[A-Za-z0-9._:/@-]{1,64}$`)
	lowercaseHexFormat    = regexp.MustCompile(`^[0-9a-f]+$`)
	ethereumAddressFormat = regexp.MustCompile(`^0x[0-9a-f]{40}$`)
	decimalStringFormat   = regexp.MustCompile(`^(0|[1-9][0-9]{0,15})(\.[0-9]{1,12})?$`)
	siweNonceFormat       = regexp.MustCompile(`^[A-Za-z0-9]{8,64}$`)
)

func ValidateIdentifier(fieldName string, value string) error {
	if !identifierFormat.MatchString(value) {
		return fmt.Errorf("%s must be 1 to 128 of letters, digits, dot, underscore, colon, dash", fieldName)
	}
	return nil
}

func validateLowercaseHexOfLength(fieldName string, value string, requiredLength int) error {
	if len(value) != requiredLength || !lowercaseHexFormat.MatchString(value) {
		return fmt.Errorf("%s must be %d lowercase hex characters without 0x", fieldName, requiredLength)
	}
	return nil
}

func ValidatePublicKeyHex(fieldName string, value string) error {
	return validateLowercaseHexOfLength(fieldName, value, lowercaseHexKeyLength)
}

func ValidateHashHex(fieldName string, value string) error {
	return validateLowercaseHexOfLength(fieldName, value, lowercaseHexHashLength)
}

func ValidateEthereumAddress(fieldName string, value string) error {
	if !ethereumAddressFormat.MatchString(value) {
		return fmt.Errorf("%s must be a 0x lowercase Ethereum address", fieldName)
	}
	return nil
}

func ValidateVaultAddress(vaultAddress string) error {
	if vaultAddress == "" {
		return nil
	}
	return ValidateEthereumAddress("vault_address", vaultAddress)
}

func ParsePositiveDecimal(fieldName string, value string) (*big.Rat, error) {
	if !decimalStringFormat.MatchString(value) {
		return nil, fmt.Errorf("%s must be a plain decimal string, no leading zeros, at most 16 digits before the point and 12 after", fieldName)
	}
	parsed, parsedOk := new(big.Rat).SetString(value)
	if !parsedOk || parsed.Sign() <= 0 {
		return nil, fmt.Errorf("%s must be above zero", fieldName)
	}
	return parsed, nil
}

func ValidateDecisionFields(requestId string, keyRef string, signerId string, decision string, reason string, intentHash string) error {
	if err := ValidateIdentifier("request_id", requestId); err != nil {
		return err
	}
	if err := ValidateIdentifier("key_ref", keyRef); err != nil {
		return err
	}
	if err := ValidateIdentifier("signer_id", signerId); err != nil {
		return err
	}
	if decision != DecisionSigned && decision != DecisionRefused {
		return errors.New("decision must be signed or refused")
	}
	if len(reason) > maximumReasonLength {
		return fmt.Errorf("reason must be at most %d bytes", maximumReasonLength)
	}
	return ValidateHashHex("intent_hash", intentHash)
}

func ValidateCommittee(committee []string, threshold uint32) error {
	if threshold < minimumCommitteeQuorum {
		return fmt.Errorf("threshold must be at least %d, one signer alone would hold the whole key", minimumCommitteeQuorum)
	}
	if int(threshold) > len(committee) {
		return errors.New("threshold cannot exceed the committee size")
	}
	seenSignerIds := make(map[string]bool, len(committee))
	for _, signerId := range committee {
		if err := ValidateIdentifier("committee member", signerId); err != nil {
			return err
		}
		if seenSignerIds[signerId] {
			return fmt.Errorf("committee member %s appears twice", signerId)
		}
		seenSignerIds[signerId] = true
	}
	return nil
}

func IsReleaseActive(activateHeight uint64, currentHeight uint64) bool {
	return currentHeight >= activateHeight
}
