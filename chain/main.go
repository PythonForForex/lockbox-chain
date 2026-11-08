package main

import (
	"errors"
	"fmt"
	"os"

	cmtcfg "github.com/cometbft/cometbft/config"
	dbm "github.com/cosmos/cosmos-db"
	"github.com/spf13/cobra"
	"github.com/spf13/viper"

	clienthelpers "cosmossdk.io/client/v2/helpers"
	"cosmossdk.io/log/v2"

	"github.com/cosmos/cosmos-sdk/client"
	clientconfig "github.com/cosmos/cosmos-sdk/client/config"
	"github.com/cosmos/cosmos-sdk/client/keys"
	"github.com/cosmos/cosmos-sdk/client/rpc"
	"github.com/cosmos/cosmos-sdk/server"
	svrcmd "github.com/cosmos/cosmos-sdk/server/cmd"
	serverconfig "github.com/cosmos/cosmos-sdk/server/config"
	servertypes "github.com/cosmos/cosmos-sdk/server/types"
	simtestutil "github.com/cosmos/cosmos-sdk/testutil/sims"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authcmd "github.com/cosmos/cosmos-sdk/x/auth/client/cli"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	genutilcli "github.com/cosmos/cosmos-sdk/x/genutil/client/cli"
)

var defaultNodeHome string

func main() {
	var err error
	defaultNodeHome, err = clienthelpers.GetNodeHomeDirectory(".lockboxd")
	if err != nil {
		panic(err)
	}
	rootCommand := buildRootCommand()
	if err := svrcmd.Execute(rootCommand, clienthelpers.EnvPrefix, defaultNodeHome); err != nil {
		_, _ = fmt.Fprintln(rootCommand.OutOrStderr(), err)
		os.Exit(1)
	}
}

func buildRootCommand() *cobra.Command {
	throwawayApp := newLockboxApp(log.NewNopLogger(), dbm.NewMemDB(), true, simtestutil.NewAppOptionsWithFlagHome(defaultNodeHome))
	baseClientContext := client.Context{}.
		WithCodec(throwawayApp.appCodec).
		WithInterfaceRegistry(throwawayApp.interfaceRegistry).
		WithTxConfig(throwawayApp.txConfig).
		WithLegacyAmino(throwawayApp.legacyAmino).
		WithInput(os.Stdin).
		WithAccountRetriever(authtypes.AccountRetriever{}).
		WithHomeDir(defaultNodeHome).
		WithViper("")

	rootCommand := &cobra.Command{
		Use:           "lockboxd",
		Short:         "Lockbox chain node",
		SilenceErrors: true,
		PersistentPreRunE: func(command *cobra.Command, _ []string) error {
			command.SetOut(command.OutOrStdout())
			command.SetErr(command.ErrOrStderr())
			clientContext, err := client.ReadPersistentCommandFlags(baseClientContext.WithCmdContext(command.Context()), command.Flags())
			if err != nil {
				return err
			}
			if clientContext, err = clientconfig.ReadFromClientConfig(clientContext); err != nil {
				return err
			}
			if err := client.SetCmdClientContextHandler(clientContext, command); err != nil {
				return err
			}
			applicationConfig := serverconfig.DefaultConfig()
			applicationConfig.MinGasPrices = "0ulockbox"
			return server.InterceptConfigsPreRunHandler(command, serverconfig.DefaultConfigTemplate, *applicationConfig, cmtcfg.DefaultConfig())
		},
	}

	sdk.GetConfig().Seal()
	basicManager := throwawayApp.basicModuleManager
	rootCommand.AddCommand(genutilcli.InitCmd(basicManager, defaultNodeHome))
	server.AddCommandsWithStartCmdOptions(rootCommand, defaultNodeHome, newApplicationForServer, exportApplicationForServer, server.StartCmdOptions{})
	rootCommand.AddCommand(
		server.StatusCommand(),
		genutilcli.Commands(throwawayApp.txConfig, basicManager, defaultNodeHome),
		buildQueryCommand(),
		buildTransactionCommand(),
		keys.Commands(),
	)

	autoCliOptions := throwawayApp.autoCliOptions()
	autoCliOptions.ClientCtx = baseClientContext
	if err := autoCliOptions.EnhanceRootCommand(rootCommand); err != nil {
		panic(err)
	}
	return rootCommand
}

func buildQueryCommand() *cobra.Command {
	command := &cobra.Command{Use: "query", Aliases: []string{"q"}, Short: "Querying subcommands", RunE: client.ValidateCmd}
	command.AddCommand(rpc.WaitTxCmd(), server.QueryBlockCmd(), authcmd.QueryTxCmd())
	return command
}

func buildTransactionCommand() *cobra.Command {
	command := &cobra.Command{Use: "tx", Short: "Transactions subcommands", RunE: client.ValidateCmd}
	command.AddCommand(
		authcmd.GetSignCommand(),
		authcmd.GetMultiSignCommand(),
		authcmd.GetValidateSignaturesCommand(),
		authcmd.GetBroadcastCommand(),
	)
	return command
}

func newApplicationForServer(logger log.Logger, db dbm.DB, applicationOptions servertypes.AppOptions) servertypes.Application {
	return newLockboxApp(logger, db, true, applicationOptions, server.DefaultBaseappOptions(applicationOptions)...)
}

func exportApplicationForServer(logger log.Logger, db dbm.DB, height int64, forZeroHeight bool, jailAllowedAddresses []string, applicationOptions servertypes.AppOptions, modulesToExport []string) (servertypes.ExportedApp, error) {
	if _, isViper := applicationOptions.(*viper.Viper); !isViper {
		return servertypes.ExportedApp{}, errors.New("application options are not viper")
	}
	if height == -1 {
		return newLockboxApp(logger, db, true, applicationOptions).ExportAppStateAndValidators(forZeroHeight, jailAllowedAddresses, modulesToExport)
	}
	app := newLockboxApp(logger, db, false, applicationOptions)
	if err := app.LoadVersion(height); err != nil {
		return servertypes.ExportedApp{}, err
	}
	return app.ExportAppStateAndValidators(forZeroHeight, jailAllowedAddresses, modulesToExport)
}
