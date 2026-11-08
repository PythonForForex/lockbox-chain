package validatorset

import (
	"context"
	"encoding/base64"
	"math/big"

	"cosmossdk.io/collections"
	"cosmossdk.io/core/store"
	errorsmod "cosmossdk.io/errors"

	"github.com/cosmos/cosmos-sdk/codec"
	sdk "github.com/cosmos/cosmos-sdk/types"
)

const (
	ModuleName             = "validatorset"
	StoreKey               = ModuleName
	ed25519PublicKeyLength = 32
	maximumValidatorPower  = int64(1_000_000_000_000)
	maximumMonikerLength   = 70
	maximumValidatorCount  = 150
)

var (
	ErrUnauthorized          = errorsmod.Register(ModuleName, 2, "unauthorized")
	ErrInvalidField          = errorsmod.Register(ModuleName, 3, "invalid field")
	ErrAlreadyExists         = errorsmod.Register(ModuleName, 4, "already exists")
	ErrNotFound              = errorsmod.Register(ModuleName, 5, "not found")
	ErrOutsiderPowerTooLarge = errorsmod.Register(ModuleName, 6, "a non-founding validator would hold one third of voting power or more")
	ErrEmptyValidatorSet     = errorsmod.Register(ModuleName, 7, "the validator set would be empty")
)

type Keeper struct {
	AdminAddress                 collections.Item[string]
	Validators                   collections.Map[string, Validator]
	PowerLastSentToConsensus     collections.Map[string, int64]
	ValidatorSetChangedThisBlock collections.Item[bool]
}

func NewKeeper(cdc codec.BinaryCodec, storeService store.KVStoreService) *Keeper {
	schemaBuilder := collections.NewSchemaBuilder(storeService)
	keeper := &Keeper{
		AdminAddress:                 collections.NewItem(schemaBuilder, collections.NewPrefix(1), "admin_address", collections.StringValue),
		Validators:                   collections.NewMap(schemaBuilder, collections.NewPrefix(2), "validators", collections.StringKey, codec.CollValue[Validator](cdc)),
		PowerLastSentToConsensus:     collections.NewMap(schemaBuilder, collections.NewPrefix(3), "power_last_sent_to_consensus", collections.StringKey, collections.Int64Value),
		ValidatorSetChangedThisBlock: collections.NewItem(schemaBuilder, collections.NewPrefix(4), "validator_set_changed_this_block", collections.BoolValue),
	}
	if _, err := schemaBuilder.Build(); err != nil {
		panic(err)
	}
	return keeper
}

func (keeper *Keeper) Admin(ctx sdk.Context) (string, error) {
	return keeper.AdminAddress.Get(ctx)
}

func (keeper *Keeper) requireAdmin(ctx context.Context, submitter string) error {
	admin, err := keeper.AdminAddress.Get(ctx)
	if err != nil {
		return err
	}
	if admin != submitter {
		return errorsmod.Wrapf(ErrUnauthorized, "%s is not the validator-set admin", submitter)
	}
	return nil
}

func decodeConsensusPublicKey(encoded string) ([]byte, error) {
	publicKeyBytes, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil || len(publicKeyBytes) != ed25519PublicKeyLength || base64.StdEncoding.EncodeToString(publicKeyBytes) != encoded {
		return nil, errorsmod.Wrap(ErrInvalidField, "consensus_public_key must be a 32-byte ed25519 key in standard base64")
	}
	return publicKeyBytes, nil
}

func validatePower(power int64) error {
	if power < 1 || power > maximumValidatorPower {
		return errorsmod.Wrapf(ErrInvalidField, "power must be 1 to %d", maximumValidatorPower)
	}
	return nil
}

func validateMoniker(moniker string) error {
	if moniker == "" || len(moniker) > maximumMonikerLength {
		return errorsmod.Wrapf(ErrInvalidField, "moniker must be 1 to %d bytes", maximumMonikerLength)
	}
	return nil
}

func validateValidator(validator Validator) error {
	if _, err := decodeConsensusPublicKey(validator.ConsensusPublicKey); err != nil {
		return err
	}
	if err := validatePower(validator.Power); err != nil {
		return err
	}
	return validateMoniker(validator.Moniker)
}

func requireSetWithinOutsiderLimit(validators []Validator) error {
	if len(validators) == 0 {
		return ErrEmptyValidatorSet
	}
	if len(validators) > maximumValidatorCount {
		return errorsmod.Wrapf(ErrInvalidField, "at most %d validators", maximumValidatorCount)
	}
	totalPower := new(big.Int)
	for _, validator := range validators {
		totalPower.Add(totalPower, big.NewInt(validator.Power))
	}
	for _, validator := range validators {
		if validator.Founding {
			continue
		}
		threeTimesPower := new(big.Int).Mul(big.NewInt(validator.Power), big.NewInt(3))
		if threeTimesPower.Cmp(totalPower) >= 0 {
			return errorsmod.Wrapf(ErrOutsiderPowerTooLarge, "%s %s would hold %d of %d", validator.Moniker, validator.ConsensusPublicKey, validator.Power, totalPower)
		}
	}
	return nil
}

func (keeper *Keeper) allValidators(ctx context.Context) ([]Validator, error) {
	validators := []Validator{}
	iterator, err := keeper.Validators.Iterate(ctx, nil)
	if err != nil {
		return nil, err
	}
	defer iterator.Close()
	for ; iterator.Valid(); iterator.Next() {
		validator, err := iterator.Value()
		if err != nil {
			return nil, err
		}
		validators = append(validators, validator)
	}
	return validators, nil
}

func (keeper *Keeper) applyChange(ctx context.Context, consensusPublicKey string, replacement *Validator) error {
	current, err := keeper.allValidators(ctx)
	if err != nil {
		return err
	}
	proposed := []Validator{}
	for _, validator := range current {
		if validator.ConsensusPublicKey != consensusPublicKey {
			proposed = append(proposed, validator)
		}
	}
	if replacement != nil {
		proposed = append(proposed, *replacement)
	}
	if err := requireSetWithinOutsiderLimit(proposed); err != nil {
		return err
	}
	if replacement == nil {
		if err := keeper.Validators.Remove(ctx, consensusPublicKey); err != nil {
			return err
		}
	} else if err := keeper.Validators.Set(ctx, consensusPublicKey, *replacement); err != nil {
		return err
	}
	return keeper.ValidatorSetChangedThisBlock.Set(ctx, true)
}

func emitValidatorSetEvent(ctx context.Context, eventType string, attributes ...sdk.Attribute) {
	sdk.UnwrapSDKContext(ctx).EventManager().EmitEvent(sdk.NewEvent("validatorset."+eventType, attributes...))
}
