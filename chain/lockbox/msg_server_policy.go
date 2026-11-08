package lockbox

import (
	"context"
	"fmt"

	"cosmossdk.io/collections"
	errorsmod "cosmossdk.io/errors"

	sdk "github.com/cosmos/cosmos-sdk/types"

	"lockbox/chain/policylogic"
)

func (keeper *Keeper) currentPolicyEpoch(ctx context.Context, keyRef string) (uint64, error) {
	epoch, err := keeper.PolicyEpochByKeyRef.Get(ctx, keyRef)
	if errorsmod.IsOf(err, collections.ErrNotFound) {
		return 0, nil
	}
	return epoch, err
}

func assetCapTerms(assetCaps []*AssetCap) []policylogic.AssetCapTerms {
	terms := make([]policylogic.AssetCapTerms, 0, len(assetCaps))
	for _, assetCap := range assetCaps {
		terms = append(terms, policylogic.AssetCapTerms{Asset: assetCap.Asset, MaxOrderSize: assetCap.MaxOrderSize, MaxDailySize: assetCap.MaxDailySize})
	}
	return terms
}

func (keeper *Keeper) requireSameVaultOnKeyRef(ctx context.Context, keyRef string, vaultAddress string) error {
	iterator, err := keeper.Policies.Iterate(ctx, collections.NewPrefixedPairRange[string, string](keyRef))
	if err != nil {
		return err
	}
	defer iterator.Close()
	for ; iterator.Valid(); iterator.Next() {
		existing, err := iterator.Value()
		if err != nil {
			return err
		}
		if existing.VaultAddress != vaultAddress {
			return errorsmod.Wrapf(ErrInvalidField, "key_ref %s is pinned to vault %q, one key_ref per sub-account", keyRef, existing.VaultAddress)
		}
	}
	return nil
}

func emitPolicyChanged(ctx context.Context, eventType string, keyRef string, delegateKey string, epoch uint64) {
	emitLockboxEvent(ctx, eventType,
		sdk.NewAttribute("key_ref", keyRef),
		sdk.NewAttribute("delegate_key", delegateKey),
		sdk.NewAttribute("epoch", fmt.Sprintf("%d", epoch)))
}

func (server *msgServer) SetPolicy(ctx context.Context, msg *MsgSetPolicy) (*MsgSetPolicyResponse, error) {
	sdkContext := sdk.UnwrapSDKContext(ctx)
	terms := policylogic.PolicyTerms{
		ChainId:      sdkContext.ChainID(),
		KeyRef:       msg.KeyRef,
		DelegateKey:  msg.DelegateKey,
		Epoch:        msg.Epoch,
		Venues:       msg.Venues,
		VaultAddress: msg.VaultAddress,
		AssetCaps:    assetCapTerms(msg.AssetCaps),
		ExpiresAt:    msg.ExpiresAt,
	}
	if err := policylogic.ValidatePolicyTerms(terms, uint64(sdkContext.BlockTime().Unix())); err != nil {
		return nil, invalidField(err)
	}
	ownerAddress, err := server.keeper.requireOwnerAddressOfKeyRef(ctx, msg.KeyRef)
	if err != nil {
		return nil, err
	}
	if err := policylogic.VerifyEip191Owner(policylogic.SetPolicyText(terms), msg.OwnerSignature, ownerAddress); err != nil {
		return nil, refusedOwnerSignature(err)
	}
	currentEpoch, err := server.keeper.currentPolicyEpoch(ctx, msg.KeyRef)
	if err != nil {
		return nil, err
	}
	if err := policylogic.RequireNextEpoch(msg.Epoch, currentEpoch); err != nil {
		return nil, errorsmod.Wrap(ErrStaleEpoch, err.Error())
	}
	if err := server.keeper.requireSameVaultOnKeyRef(ctx, msg.KeyRef, msg.VaultAddress); err != nil {
		return nil, err
	}
	policy := Policy{
		KeyRef:       msg.KeyRef,
		DelegateKey:  msg.DelegateKey,
		Epoch:        msg.Epoch,
		Venues:       msg.Venues,
		VaultAddress: msg.VaultAddress,
		AssetCaps:    msg.AssetCaps,
		ExpiresAt:    msg.ExpiresAt,
		Revoked:      false,
	}
	if err := server.keeper.Policies.Set(ctx, collections.Join(msg.KeyRef, msg.DelegateKey), policy); err != nil {
		return nil, err
	}
	if err := server.keeper.PolicyEpochByKeyRef.Set(ctx, msg.KeyRef, msg.Epoch); err != nil {
		return nil, err
	}
	emitPolicyChanged(ctx, "policy_set", msg.KeyRef, msg.DelegateKey, msg.Epoch)
	return &MsgSetPolicyResponse{Epoch: msg.Epoch}, nil
}

func (server *msgServer) RevokePolicy(ctx context.Context, msg *MsgRevokePolicy) (*MsgRevokePolicyResponse, error) {
	policyKey := collections.Join(msg.KeyRef, msg.DelegateKey)
	policy, err := server.keeper.Policies.Get(ctx, policyKey)
	if err != nil {
		return nil, errorsmod.Wrapf(ErrNotFound, "policy %s %s", msg.KeyRef, msg.DelegateKey)
	}
	ownerAddress, err := server.keeper.requireOwnerAddressOfKeyRef(ctx, msg.KeyRef)
	if err != nil {
		return nil, err
	}
	revocationText := policylogic.RevokePolicyText(policylogic.PolicyRevocation{
		ChainId:     sdk.UnwrapSDKContext(ctx).ChainID(),
		KeyRef:      msg.KeyRef,
		DelegateKey: msg.DelegateKey,
		Epoch:       msg.Epoch,
	})
	if err := policylogic.VerifyEip191Owner(revocationText, msg.OwnerSignature, ownerAddress); err != nil {
		return nil, refusedOwnerSignature(err)
	}
	currentEpoch, err := server.keeper.currentPolicyEpoch(ctx, msg.KeyRef)
	if err != nil {
		return nil, err
	}
	if err := policylogic.RequireNextEpoch(msg.Epoch, currentEpoch); err != nil {
		return nil, errorsmod.Wrap(ErrStaleEpoch, err.Error())
	}
	policy.Epoch = msg.Epoch
	if !policy.Revoked {
		policy.RevokedAt = uint64(sdk.UnwrapSDKContext(ctx).BlockTime().Unix())
	}
	policy.Revoked = true
	if err := server.keeper.Policies.Set(ctx, policyKey, policy); err != nil {
		return nil, err
	}
	if err := server.keeper.PolicyEpochByKeyRef.Set(ctx, msg.KeyRef, msg.Epoch); err != nil {
		return nil, err
	}
	emitPolicyChanged(ctx, "policy_revoked", msg.KeyRef, msg.DelegateKey, msg.Epoch)
	return &MsgRevokePolicyResponse{Epoch: msg.Epoch}, nil
}
