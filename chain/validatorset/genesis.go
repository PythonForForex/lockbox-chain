package validatorset

import (
	"context"

	abci "github.com/cometbft/cometbft/abci/types"

	errorsmod "cosmossdk.io/errors"

	sdk "github.com/cosmos/cosmos-sdk/types"
)

func DefaultGenesisState() *GenesisState {
	return &GenesisState{}
}

func ValidateGenesisState(state *GenesisState) error {
	if _, err := sdk.AccAddressFromBech32(state.Admin); err != nil {
		return errorsmod.Wrapf(ErrInvalidField, "admin %q: %v", state.Admin, err)
	}
	seenKeys := map[string]bool{}
	validators := make([]Validator, 0, len(state.Validators))
	for _, validator := range state.Validators {
		if validator == nil {
			return errorsmod.Wrap(ErrInvalidField, "empty validator entry")
		}
		if err := validateValidator(*validator); err != nil {
			return err
		}
		if seenKeys[validator.ConsensusPublicKey] {
			return errorsmod.Wrapf(ErrAlreadyExists, "validator %s twice in genesis", validator.ConsensusPublicKey)
		}
		seenKeys[validator.ConsensusPublicKey] = true
		validators = append(validators, *validator)
	}
	return requireSetWithinOutsiderLimit(validators)
}

func (keeper *Keeper) InitializeFromGenesis(ctx context.Context, state *GenesisState) ([]abci.ValidatorUpdate, error) {
	if err := ValidateGenesisState(state); err != nil {
		return nil, err
	}
	if err := keeper.AdminAddress.Set(ctx, state.Admin); err != nil {
		return nil, err
	}
	updates := []abci.ValidatorUpdate{}
	for _, validator := range state.Validators {
		if err := keeper.Validators.Set(ctx, validator.ConsensusPublicKey, *validator); err != nil {
			return nil, err
		}
		if err := keeper.PowerLastSentToConsensus.Set(ctx, validator.ConsensusPublicKey, validator.Power); err != nil {
			return nil, err
		}
		update, err := consensusUpdate(validator.ConsensusPublicKey, validator.Power)
		if err != nil {
			return nil, err
		}
		updates = append(updates, update)
	}
	if err := keeper.ValidatorSetChangedThisBlock.Set(ctx, false); err != nil {
		return nil, err
	}
	return updates, nil
}

func (keeper *Keeper) ExportToGenesis(ctx context.Context) (*GenesisState, error) {
	admin, err := keeper.AdminAddress.Get(ctx)
	if err != nil {
		return nil, err
	}
	validators, err := keeper.allValidators(ctx)
	if err != nil {
		return nil, err
	}
	state := &GenesisState{Admin: admin}
	for index := range validators {
		state.Validators = append(state.Validators, &validators[index])
	}
	return state, nil
}
