package validatorset

import (
	"context"
	"encoding/json"

	abci "github.com/cometbft/cometbft/abci/types"
	gwruntime "github.com/grpc-ecosystem/grpc-gateway/runtime"

	autocliv1 "cosmossdk.io/api/cosmos/autocli/v1"
	"cosmossdk.io/core/appmodule"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/codec"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/types/module"
	"github.com/cosmos/cosmos-sdk/types/msgservice"
)

var (
	_ module.AppModuleBasic      = AppModule{}
	_ appmodule.AppModule        = AppModule{}
	_ module.HasServices         = AppModule{}
	_ module.HasABCIGenesis      = AppModule{}
	_ module.HasABCIEndBlock     = AppModule{}
	_ module.HasConsensusVersion = AppModule{}
)

type AppModule struct {
	keeper *Keeper
}

func NewAppModule(keeper *Keeper) AppModule {
	return AppModule{keeper: keeper}
}

func (AppModule) Name() string { return ModuleName }

func (AppModule) IsAppModule() {}

func (AppModule) IsOnePerModuleType() {}

func (AppModule) ConsensusVersion() uint64 { return 1 }

func (AppModule) RegisterLegacyAminoCodec(*codec.LegacyAmino) {}

func (AppModule) RegisterGRPCGatewayRoutes(client.Context, *gwruntime.ServeMux) {}

func (AppModule) RegisterInterfaces(registry codectypes.InterfaceRegistry) {
	registry.RegisterImplementations((*sdk.Msg)(nil),
		&MsgAddValidator{},
		&MsgSetValidatorPower{},
		&MsgRemoveValidator{},
	)
	msgservice.RegisterMsgServiceDesc(registry, &Msg_serviceDesc)
}

func (appModule AppModule) RegisterServices(configurator module.Configurator) {
	RegisterMsgServer(configurator.MsgServer(), NewMsgServer(appModule.keeper))
	RegisterQueryServer(configurator.QueryServer(), NewQueryServer(appModule.keeper))
}

func (AppModule) DefaultGenesis(cdc codec.JSONCodec) json.RawMessage {
	return cdc.MustMarshalJSON(DefaultGenesisState())
}

func (AppModule) ValidateGenesis(cdc codec.JSONCodec, _ client.TxEncodingConfig, rawState json.RawMessage) error {
	var state GenesisState
	if err := cdc.UnmarshalJSON(rawState, &state); err != nil {
		return err
	}
	return ValidateGenesisState(&state)
}

func (appModule AppModule) InitGenesis(ctx sdk.Context, cdc codec.JSONCodec, rawState json.RawMessage) []abci.ValidatorUpdate {
	var state GenesisState
	cdc.MustUnmarshalJSON(rawState, &state)
	updates, err := appModule.keeper.InitializeFromGenesis(context.Context(ctx), &state)
	if err != nil {
		panic(err)
	}
	return updates
}

func (appModule AppModule) ExportGenesis(ctx sdk.Context, cdc codec.JSONCodec) json.RawMessage {
	state, err := appModule.keeper.ExportToGenesis(context.Context(ctx))
	if err != nil {
		panic(err)
	}
	return cdc.MustMarshalJSON(state)
}

func (appModule AppModule) EndBlock(ctx context.Context) ([]abci.ValidatorUpdate, error) {
	return appModule.keeper.UpdatesForConsensus(ctx)
}

func (AppModule) AutoCLIOptions() *autocliv1.ModuleOptions {
	return &autocliv1.ModuleOptions{
		Tx:    &autocliv1.ServiceCommandDescriptor{Service: "lockbox.validatorset.v1.Msg"},
		Query: &autocliv1.ServiceCommandDescriptor{Service: "lockbox.validatorset.v1.Query"},
	}
}
