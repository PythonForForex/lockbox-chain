package main

import (
	"encoding/json"

	cmtproto "github.com/cometbft/cometbft/proto/tendermint/types"

	servertypes "github.com/cosmos/cosmos-sdk/server/types"
)

func (app *LockboxApp) ExportAppStateAndValidators(forZeroHeight bool, _ []string, modulesToExport []string) (servertypes.ExportedApp, error) {
	ctx := app.NewContextLegacy(true, cmtproto.Header{Height: app.LastBlockHeight()})
	height := app.LastBlockHeight() + 1
	if forZeroHeight {
		height = 0
		ctx = ctx.WithBlockHeight(height)
	}
	genesisState, err := app.moduleManager.ExportGenesisForModules(ctx, app.appCodec, modulesToExport)
	if err != nil {
		return servertypes.ExportedApp{}, err
	}
	appState, err := json.MarshalIndent(genesisState, "", "  ")
	if err != nil {
		return servertypes.ExportedApp{}, err
	}
	validators, err := app.validatorSetKeeper.GenesisValidatorsForExport(ctx)
	if err != nil {
		return servertypes.ExportedApp{}, err
	}
	return servertypes.ExportedApp{
		AppState:        appState,
		Validators:      validators,
		Height:          height,
		ConsensusParams: app.GetConsensusParams(ctx),
	}, nil
}
