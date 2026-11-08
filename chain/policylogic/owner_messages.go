package policylogic

import (
	"errors"
	"fmt"
	"strings"
	"time"
)

type OwnerClaim struct {
	ChainId         string
	AccountId       string
	RegistrationKey string
	OwnerAddress    string
	Nonce           string
	IssuedAt        uint64
	ExpiresAt       uint64
}

type KeyRefRegistration struct {
	ChainId      string
	KeyRef       string
	AccountId    string
	VenueAddress string
	Committee    []string
	Threshold    uint32
}

const dailySizeMeaningLine = "daily size is per delegate and counts signed size, not fills, including failed rounds"

type AssetCapTerms struct {
	Asset        string
	MaxOrderSize string
	MaxDailySize string
}

type PolicyTerms struct {
	ChainId      string
	KeyRef       string
	DelegateKey  string
	Epoch        uint64
	Venues       []string
	VaultAddress string
	AssetCaps    []AssetCapTerms
	ExpiresAt    uint64
}

type PolicyRevocation struct {
	ChainId     string
	KeyRef      string
	DelegateKey string
	Epoch       uint64
}

func formatSiweTime(unixSeconds uint64) string {
	return time.Unix(int64(unixSeconds), 0).UTC().Format(time.RFC3339)
}

func OwnerClaimText(claim OwnerClaim) (string, error) {
	checksummedAddress, err := ChecksummedAddress(claim.OwnerAddress)
	if err != nil {
		return "", err
	}
	return "lockbox wants you to sign in with your Ethereum account:\n" +
		checksummedAddress + "\n\n" +
		"Claim lockbox account " + claim.AccountId + " with registration key " + claim.RegistrationKey + "\n\n" +
		"URI: lockbox://" + claim.ChainId + "/claim\n" +
		"Version: 1\n" +
		"Chain ID: 1\n" +
		"Nonce: " + claim.Nonce + "\n" +
		"Issued At: " + formatSiweTime(claim.IssuedAt) + "\n" +
		"Expiration Time: " + formatSiweTime(claim.ExpiresAt), nil
}

func ValidateOwnerClaimWindow(claim OwnerClaim, blockTimeSeconds uint64) error {
	if !siweNonceFormat.MatchString(claim.Nonce) {
		return errors.New("nonce must be 8 to 64 letters or digits")
	}
	if claim.ExpiresAt <= claim.IssuedAt {
		return errors.New("expires_at must be after issued_at")
	}
	if blockTimeSeconds >= claim.ExpiresAt {
		return errors.New("claim message has expired")
	}
	return nil
}

func KeyRefRegistrationText(registration KeyRefRegistration) string {
	return "lockbox register key ref\n" +
		"chain id: " + registration.ChainId + "\n" +
		"key ref: " + registration.KeyRef + "\n" +
		"account id: " + registration.AccountId + "\n" +
		"venue address: " + registration.VenueAddress + "\n" +
		"committee: " + strings.Join(registration.Committee, ",") + "\n" +
		fmt.Sprintf("threshold: %d", registration.Threshold)
}

func vaultOrNone(vaultAddress string) string {
	if vaultAddress == "" {
		return "none"
	}
	return vaultAddress
}

func assetCapLines(assetCaps []AssetCapTerms) string {
	var lines strings.Builder
	for _, assetCap := range assetCaps {
		lines.WriteString("asset " + assetCap.Asset + ": max order size " + assetCap.MaxOrderSize + ", max daily size " + assetCap.MaxDailySize + "\n")
	}
	return lines.String()
}

func SetPolicyText(terms PolicyTerms) string {
	return "lockbox set policy\n" +
		"chain id: " + terms.ChainId + "\n" +
		"key ref: " + terms.KeyRef + "\n" +
		"delegate key: " + terms.DelegateKey + "\n" +
		fmt.Sprintf("epoch: %d\n", terms.Epoch) +
		"venues: " + strings.Join(terms.Venues, ",") + "\n" +
		"vault address: " + vaultOrNone(terms.VaultAddress) + "\n" +
		assetCapLines(terms.AssetCaps) +
		dailySizeMeaningLine + "\n" +
		fmt.Sprintf("expires at: %d", terms.ExpiresAt)
}

func RevokePolicyText(revocation PolicyRevocation) string {
	return "lockbox revoke policy\n" +
		"chain id: " + revocation.ChainId + "\n" +
		"key ref: " + revocation.KeyRef + "\n" +
		"delegate key: " + revocation.DelegateKey + "\n" +
		fmt.Sprintf("epoch: %d", revocation.Epoch)
}

func ValidatePolicyTerms(terms PolicyTerms, blockTimeSeconds uint64) error {
	if err := ValidateIdentifier("key_ref", terms.KeyRef); err != nil {
		return err
	}
	if err := ValidatePublicKeyHex("delegate_key", terms.DelegateKey); err != nil {
		return err
	}
	if len(terms.Venues) == 0 {
		return errors.New("venues cannot be empty, only assets may mean any")
	}
	for _, venue := range terms.Venues {
		if err := ValidateIdentifier("venue", venue); err != nil {
			return err
		}
	}
	if err := ValidateVaultAddress(terms.VaultAddress); err != nil {
		return err
	}
	if len(terms.AssetCaps) == 0 {
		return errors.New("asset_caps cannot be empty, every allowed asset names its own caps")
	}
	seenAssets := make(map[string]bool, len(terms.AssetCaps))
	for _, assetCap := range terms.AssetCaps {
		if !assetNameFormat.MatchString(assetCap.Asset) {
			return fmt.Errorf("asset %q has characters that are not allowed", assetCap.Asset)
		}
		if seenAssets[assetCap.Asset] {
			return fmt.Errorf("asset %s appears twice", assetCap.Asset)
		}
		seenAssets[assetCap.Asset] = true
		maxOrderSize, err := ParsePositiveDecimal("max_order_size", assetCap.MaxOrderSize)
		if err != nil {
			return err
		}
		maxDailySize, err := ParsePositiveDecimal("max_daily_size", assetCap.MaxDailySize)
		if err != nil {
			return err
		}
		if maxOrderSize.Cmp(maxDailySize) > 0 {
			return fmt.Errorf("asset %s: max_order_size cannot exceed max_daily_size", assetCap.Asset)
		}
	}
	if terms.ExpiresAt <= blockTimeSeconds {
		return errors.New("expires_at is already in the past")
	}
	return nil
}

func RequireNextEpoch(submittedEpoch uint64, currentEpoch uint64) error {
	if submittedEpoch != currentEpoch+1 {
		return fmt.Errorf("epoch must be %d, the next one for this key_ref, got %d", currentEpoch+1, submittedEpoch)
	}
	return nil
}
