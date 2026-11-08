package validatorset

import (
	"context"

	errorsmod "cosmossdk.io/errors"
)

type queryServer struct {
	keeper *Keeper
}

var _ QueryServer = queryServer{}

func NewQueryServer(keeper *Keeper) QueryServer {
	return queryServer{keeper: keeper}
}

func (server queryServer) Admin(ctx context.Context, _ *QueryAdminRequest) (*QueryAdminResponse, error) {
	admin, err := server.keeper.AdminAddress.Get(ctx)
	if err != nil {
		return nil, err
	}
	return &QueryAdminResponse{Admin: admin}, nil
}

func (server queryServer) Validators(ctx context.Context, _ *QueryValidatorsRequest) (*QueryValidatorsResponse, error) {
	validators, err := server.keeper.allValidators(ctx)
	if err != nil {
		return nil, err
	}
	response := &QueryValidatorsResponse{}
	for index := range validators {
		response.Validators = append(response.Validators, &validators[index])
		response.TotalPower += validators[index].Power
	}
	return response, nil
}

func (server queryServer) Validator(ctx context.Context, request *QueryValidatorRequest) (*QueryValidatorResponse, error) {
	validator, err := server.keeper.Validators.Get(ctx, request.ConsensusPublicKey)
	if err != nil {
		return nil, errorsmod.Wrapf(ErrNotFound, "validator %s", request.ConsensusPublicKey)
	}
	return &QueryValidatorResponse{Validator: &validator}, nil
}
