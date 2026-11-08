package lockbox

import (
	"context"

	"cosmossdk.io/collections"
)

func DefaultGenesisState() *GenesisState {
	return &GenesisState{ReleaseActivationDelayBlocks: defaultReleaseActivationDelayBlocks}
}

func (keeper *Keeper) InitializeFromGenesis(ctx context.Context, state *GenesisState) error {
	if err := keeper.ReleaseActivationDelayBlocks.Set(ctx, state.ReleaseActivationDelayBlocks); err != nil {
		return err
	}
	for _, account := range state.Accounts {
		if err := keeper.Accounts.Set(ctx, account.AccountId, *account); err != nil {
			return err
		}
	}
	for _, signer := range state.Signers {
		if err := keeper.Signers.Set(ctx, signer.SignerId, *signer); err != nil {
			return err
		}
		if err := keeper.SignerIdByAddress.Set(ctx, signer.SignerAddress, signer.SignerId); err != nil {
			return err
		}
	}
	for _, keyRef := range state.KeyRefs {
		if err := keeper.KeyRefs.Set(ctx, keyRef.KeyRef, *keyRef); err != nil {
			return err
		}
	}
	for _, keyRefEpoch := range state.KeyRefPolicyEpochs {
		if err := keeper.PolicyEpochByKeyRef.Set(ctx, keyRefEpoch.KeyRef, keyRefEpoch.PolicyEpoch); err != nil {
			return err
		}
	}
	for _, policy := range state.Policies {
		if err := keeper.Policies.Set(ctx, collections.Join(policy.KeyRef, policy.DelegateKey), *policy); err != nil {
			return err
		}
	}
	for _, release := range state.AllowedReleases {
		if err := keeper.AllowedReleases.Set(ctx, release.BinaryHash, *release); err != nil {
			return err
		}
	}
	for sequence, entry := range state.LogEntries {
		if err := keeper.LogEntries.Set(ctx, uint64(sequence), *entry); err != nil {
			return err
		}
	}
	if err := keeper.NextLogSequence.Set(ctx, uint64(len(state.LogEntries))); err != nil {
		return err
	}
	for _, anchor := range state.TapeAnchors {
		if err := keeper.TapeAnchors.Set(ctx, anchor.SegmentId, *anchor); err != nil {
			return err
		}
	}
	return nil
}

func collectAllValues[Key any, Value any](ctx context.Context, values collections.Map[Key, Value]) ([]*Value, error) {
	collected := []*Value{}
	err := values.Walk(ctx, nil, func(_ Key, value Value) (bool, error) {
		collected = append(collected, &value)
		return false, nil
	})
	return collected, err
}

func (keeper *Keeper) ExportToGenesis(ctx context.Context) (*GenesisState, error) {
	delayBlocks, err := keeper.ReleaseActivationDelayBlocks.Get(ctx)
	if err != nil {
		return nil, err
	}
	state := &GenesisState{ReleaseActivationDelayBlocks: delayBlocks}
	if state.Accounts, err = collectAllValues(ctx, keeper.Accounts); err != nil {
		return nil, err
	}
	if state.Signers, err = collectAllValues(ctx, keeper.Signers); err != nil {
		return nil, err
	}
	if state.KeyRefs, err = collectAllValues(ctx, keeper.KeyRefs); err != nil {
		return nil, err
	}
	if state.Policies, err = collectAllValues(ctx, keeper.Policies); err != nil {
		return nil, err
	}
	if state.AllowedReleases, err = collectAllValues(ctx, keeper.AllowedReleases); err != nil {
		return nil, err
	}
	if state.LogEntries, err = collectAllValues(ctx, keeper.LogEntries); err != nil {
		return nil, err
	}
	if state.TapeAnchors, err = collectAllValues(ctx, keeper.TapeAnchors); err != nil {
		return nil, err
	}
	err = keeper.PolicyEpochByKeyRef.Walk(ctx, nil, func(keyRef string, policyEpoch uint64) (bool, error) {
		state.KeyRefPolicyEpochs = append(state.KeyRefPolicyEpochs, &KeyRefPolicyEpoch{KeyRef: keyRef, PolicyEpoch: policyEpoch})
		return false, nil
	})
	return state, err
}
