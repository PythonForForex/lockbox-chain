package lockbox

import (
	"context"

	"cosmossdk.io/collections"
	"cosmossdk.io/core/store"
	errorsmod "cosmossdk.io/errors"

	"github.com/cosmos/cosmos-sdk/codec"
	sdk "github.com/cosmos/cosmos-sdk/types"
)

const (
	ModuleName                          = "lockbox"
	StoreKey                            = ModuleName
	defaultReleaseActivationDelayBlocks = 100
)

var (
	ErrNotFound      = errorsmod.Register(ModuleName, 2, "not found")
	ErrAlreadyExists = errorsmod.Register(ModuleName, 3, "already exists")
	ErrUnauthorized  = errorsmod.Register(ModuleName, 4, "unauthorized")
	ErrInvalidField  = errorsmod.Register(ModuleName, 5, "invalid field")
	ErrStaleEpoch    = errorsmod.Register(ModuleName, 6, "stale epoch")
	ErrNotClaimed    = errorsmod.Register(ModuleName, 7, "account not claimed by an owner")
)

type ValidatorAdminSource interface {
	Admin(ctx sdk.Context) (string, error)
}

type Keeper struct {
	validatorAdminSource         ValidatorAdminSource
	Accounts                     collections.Map[string, Account]
	Signers                      collections.Map[string, RegisteredSigner]
	SignerIdByAddress            collections.Map[string, string]
	KeyRefs                      collections.Map[string, KeyRef]
	PolicyEpochByKeyRef          collections.Map[string, uint64]
	Policies                     collections.Map[collections.Pair[string, string], Policy]
	AllowedReleases              collections.Map[string, AllowedRelease]
	LogEntries                   collections.Map[uint64, LogEntry]
	NextLogSequence              collections.Sequence
	TapeAnchors                  collections.Map[string, TapeAnchor]
	ReleaseActivationDelayBlocks collections.Item[uint64]
}

func NewKeeper(cdc codec.BinaryCodec, storeService store.KVStoreService, validatorAdminSource ValidatorAdminSource) *Keeper {
	schemaBuilder := collections.NewSchemaBuilder(storeService)
	keeper := &Keeper{
		validatorAdminSource:         validatorAdminSource,
		Accounts:                     collections.NewMap(schemaBuilder, collections.NewPrefix(1), "accounts", collections.StringKey, codec.CollValue[Account](cdc)),
		Signers:                      collections.NewMap(schemaBuilder, collections.NewPrefix(2), "signers", collections.StringKey, codec.CollValue[RegisteredSigner](cdc)),
		SignerIdByAddress:            collections.NewMap(schemaBuilder, collections.NewPrefix(3), "signer_id_by_address", collections.StringKey, collections.StringValue),
		KeyRefs:                      collections.NewMap(schemaBuilder, collections.NewPrefix(4), "key_refs", collections.StringKey, codec.CollValue[KeyRef](cdc)),
		PolicyEpochByKeyRef:          collections.NewMap(schemaBuilder, collections.NewPrefix(5), "policy_epoch_by_key_ref", collections.StringKey, collections.Uint64Value),
		Policies:                     collections.NewMap(schemaBuilder, collections.NewPrefix(6), "policies", collections.PairKeyCodec(collections.StringKey, collections.StringKey), codec.CollValue[Policy](cdc)),
		AllowedReleases:              collections.NewMap(schemaBuilder, collections.NewPrefix(7), "allowed_releases", collections.StringKey, codec.CollValue[AllowedRelease](cdc)),
		LogEntries:                   collections.NewMap(schemaBuilder, collections.NewPrefix(8), "log_entries", collections.Uint64Key, codec.CollValue[LogEntry](cdc)),
		NextLogSequence:              collections.NewSequence(schemaBuilder, collections.NewPrefix(9), "next_log_sequence"),
		TapeAnchors:                  collections.NewMap(schemaBuilder, collections.NewPrefix(10), "tape_anchors", collections.StringKey, codec.CollValue[TapeAnchor](cdc)),
		ReleaseActivationDelayBlocks: collections.NewItem(schemaBuilder, collections.NewPrefix(11), "release_activation_delay_blocks", collections.Uint64Value),
	}
	if _, err := schemaBuilder.Build(); err != nil {
		panic(err)
	}
	return keeper
}

func (keeper *Keeper) requireValidatorAdmin(ctx context.Context, submitter string) error {
	admin, err := keeper.validatorAdminSource.Admin(sdk.UnwrapSDKContext(ctx))
	if err != nil {
		return err
	}
	if admin != submitter {
		return errorsmod.Wrapf(ErrUnauthorized, "%s is not the validator-set admin", submitter)
	}
	return nil
}

func (keeper *Keeper) requireOwnerAddressOfAccount(ctx context.Context, accountId string) (string, error) {
	account, err := keeper.Accounts.Get(ctx, accountId)
	if err != nil {
		return "", errorsmod.Wrapf(ErrNotFound, "account %s", accountId)
	}
	if account.OwnerAddress == "" {
		return "", errorsmod.Wrapf(ErrNotClaimed, "account %s", accountId)
	}
	return account.OwnerAddress, nil
}

func (keeper *Keeper) requireOwnerAddressOfKeyRef(ctx context.Context, keyRef string) (string, error) {
	storedKeyRef, err := keeper.KeyRefs.Get(ctx, keyRef)
	if err != nil {
		return "", errorsmod.Wrapf(ErrNotFound, "key_ref %s", keyRef)
	}
	return keeper.requireOwnerAddressOfAccount(ctx, storedKeyRef.AccountId)
}

func emitLockboxEvent(ctx context.Context, eventType string, attributes ...sdk.Attribute) {
	sdk.UnwrapSDKContext(ctx).EventManager().EmitEvent(sdk.NewEvent("lockbox."+eventType, attributes...))
}
