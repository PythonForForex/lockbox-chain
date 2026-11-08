package validatorset

import (
	"context"
	"errors"
	"sort"

	abci "github.com/cometbft/cometbft/abci/types"
	cmted25519 "github.com/cometbft/cometbft/crypto/ed25519"
	cmtprotocrypto "github.com/cometbft/cometbft/proto/tendermint/crypto"
	cmttypes "github.com/cometbft/cometbft/types"

	"cosmossdk.io/collections"

	sdk "github.com/cosmos/cosmos-sdk/types"
)

func consensusUpdate(consensusPublicKey string, power int64) (abci.ValidatorUpdate, error) {
	publicKeyBytes, err := decodeConsensusPublicKey(consensusPublicKey)
	if err != nil {
		return abci.ValidatorUpdate{}, err
	}
	return abci.ValidatorUpdate{
		PubKey: cmtprotocrypto.PublicKey{Sum: &cmtprotocrypto.PublicKey_Ed25519{Ed25519: publicKeyBytes}},
		Power:  power,
	}, nil
}

func (keeper *Keeper) UpdatesForConsensus(ctx context.Context) ([]abci.ValidatorUpdate, error) {
	changed, err := keeper.ValidatorSetChangedThisBlock.Get(ctx)
	if errors.Is(err, collections.ErrNotFound) || (err == nil && !changed) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	current, err := keeper.allValidators(ctx)
	if err != nil {
		return nil, err
	}
	desiredPowerByKey := map[string]int64{}
	for _, validator := range current {
		desiredPowerByKey[validator.ConsensusPublicKey] = validator.Power
	}
	sentPowerByKey := map[string]int64{}
	iterator, err := keeper.PowerLastSentToConsensus.Iterate(ctx, nil)
	if err != nil {
		return nil, err
	}
	for ; iterator.Valid(); iterator.Next() {
		entry, err := iterator.KeyValue()
		if err != nil {
			iterator.Close()
			return nil, err
		}
		sentPowerByKey[entry.Key] = entry.Value
	}
	iterator.Close()

	changedKeys := []string{}
	for key, power := range desiredPowerByKey {
		if sentPowerByKey[key] != power {
			changedKeys = append(changedKeys, key)
		}
	}
	for key := range sentPowerByKey {
		if _, stillPresent := desiredPowerByKey[key]; !stillPresent {
			changedKeys = append(changedKeys, key)
		}
	}
	sort.Strings(changedKeys)

	updates := []abci.ValidatorUpdate{}
	for _, key := range changedKeys {
		power := desiredPowerByKey[key]
		update, err := consensusUpdate(key, power)
		if err != nil {
			return nil, err
		}
		updates = append(updates, update)
		if power == 0 {
			err = keeper.PowerLastSentToConsensus.Remove(ctx, key)
		} else {
			err = keeper.PowerLastSentToConsensus.Set(ctx, key, power)
		}
		if err != nil {
			return nil, err
		}
	}
	if err := keeper.ValidatorSetChangedThisBlock.Set(ctx, false); err != nil {
		return nil, err
	}
	return updates, nil
}

func (keeper *Keeper) GenesisValidatorsForExport(ctx sdk.Context) ([]cmttypes.GenesisValidator, error) {
	validators, err := keeper.allValidators(ctx)
	if err != nil {
		return nil, err
	}
	genesisValidators := make([]cmttypes.GenesisValidator, 0, len(validators))
	for _, validator := range validators {
		publicKeyBytes, err := decodeConsensusPublicKey(validator.ConsensusPublicKey)
		if err != nil {
			return nil, err
		}
		publicKey := cmted25519.PubKey(publicKeyBytes)
		genesisValidators = append(genesisValidators, cmttypes.GenesisValidator{
			Address: publicKey.Address(),
			PubKey:  publicKey,
			Power:   validator.Power,
			Name:    validator.Moniker,
		})
	}
	return genesisValidators, nil
}
