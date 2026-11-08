package validatorset

import (
	"context"
	"strconv"

	errorsmod "cosmossdk.io/errors"

	sdk "github.com/cosmos/cosmos-sdk/types"
)

type msgServer struct {
	keeper *Keeper
}

var _ MsgServer = msgServer{}

func NewMsgServer(keeper *Keeper) MsgServer {
	return msgServer{keeper: keeper}
}

func (server msgServer) AddValidator(ctx context.Context, message *MsgAddValidator) (*MsgAddValidatorResponse, error) {
	if err := server.keeper.requireAdmin(ctx, message.Admin); err != nil {
		return nil, err
	}
	validator := Validator{ConsensusPublicKey: message.ConsensusPublicKey, Power: message.Power, Moniker: message.Moniker, Founding: message.Founding}
	if err := validateValidator(validator); err != nil {
		return nil, err
	}
	alreadyPresent, err := server.keeper.Validators.Has(ctx, message.ConsensusPublicKey)
	if err != nil {
		return nil, err
	}
	if alreadyPresent {
		return nil, errorsmod.Wrapf(ErrAlreadyExists, "validator %s", message.ConsensusPublicKey)
	}
	if err := server.keeper.applyChange(ctx, message.ConsensusPublicKey, &validator); err != nil {
		return nil, err
	}
	emitValidatorSetEvent(ctx, "validator_added",
		sdk.NewAttribute("consensus_public_key", validator.ConsensusPublicKey),
		sdk.NewAttribute("moniker", validator.Moniker),
		sdk.NewAttribute("power", strconv.FormatInt(validator.Power, 10)),
		sdk.NewAttribute("founding", strconv.FormatBool(validator.Founding)))
	return &MsgAddValidatorResponse{}, nil
}

func (server msgServer) SetValidatorPower(ctx context.Context, message *MsgSetValidatorPower) (*MsgSetValidatorPowerResponse, error) {
	if err := server.keeper.requireAdmin(ctx, message.Admin); err != nil {
		return nil, err
	}
	if err := validatePower(message.Power); err != nil {
		return nil, err
	}
	validator, err := server.keeper.Validators.Get(ctx, message.ConsensusPublicKey)
	if err != nil {
		return nil, errorsmod.Wrapf(ErrNotFound, "validator %s", message.ConsensusPublicKey)
	}
	validator.Power = message.Power
	if err := server.keeper.applyChange(ctx, message.ConsensusPublicKey, &validator); err != nil {
		return nil, err
	}
	emitValidatorSetEvent(ctx, "validator_power_set",
		sdk.NewAttribute("consensus_public_key", validator.ConsensusPublicKey),
		sdk.NewAttribute("power", strconv.FormatInt(validator.Power, 10)))
	return &MsgSetValidatorPowerResponse{}, nil
}

func (server msgServer) RemoveValidator(ctx context.Context, message *MsgRemoveValidator) (*MsgRemoveValidatorResponse, error) {
	if err := server.keeper.requireAdmin(ctx, message.Admin); err != nil {
		return nil, err
	}
	present, err := server.keeper.Validators.Has(ctx, message.ConsensusPublicKey)
	if err != nil {
		return nil, err
	}
	if !present {
		return nil, errorsmod.Wrapf(ErrNotFound, "validator %s", message.ConsensusPublicKey)
	}
	if err := server.keeper.applyChange(ctx, message.ConsensusPublicKey, nil); err != nil {
		return nil, err
	}
	emitValidatorSetEvent(ctx, "validator_removed", sdk.NewAttribute("consensus_public_key", message.ConsensusPublicKey))
	return &MsgRemoveValidatorResponse{}, nil
}
