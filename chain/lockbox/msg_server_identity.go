package lockbox

import (
	"context"

	errorsmod "cosmossdk.io/errors"

	sdk "github.com/cosmos/cosmos-sdk/types"

	"lockbox/chain/policylogic"
)

type msgServer struct {
	keeper *Keeper
}

func NewMsgServer(keeper *Keeper) MsgServer {
	return &msgServer{keeper: keeper}
}

func invalidField(err error) error {
	return errorsmod.Wrap(ErrInvalidField, err.Error())
}

func refusedOwnerSignature(err error) error {
	return errorsmod.Wrap(ErrUnauthorized, err.Error())
}

func (server *msgServer) RegisterSigner(ctx context.Context, msg *MsgRegisterSigner) (*MsgRegisterSignerResponse, error) {
	if err := server.keeper.requireValidatorAdmin(ctx, msg.Submitter); err != nil {
		return nil, err
	}
	if err := policylogic.ValidateIdentifier("signer_id", msg.SignerId); err != nil {
		return nil, invalidField(err)
	}
	if _, err := sdk.AccAddressFromBech32(msg.SignerAddress); err != nil {
		return nil, invalidField(err)
	}
	if exists, err := server.keeper.Signers.Has(ctx, msg.SignerId); err != nil {
		return nil, err
	} else if exists {
		return nil, errorsmod.Wrapf(ErrAlreadyExists, "signer %s", msg.SignerId)
	}
	if exists, err := server.keeper.SignerIdByAddress.Has(ctx, msg.SignerAddress); err != nil {
		return nil, err
	} else if exists {
		return nil, errorsmod.Wrapf(ErrAlreadyExists, "signer address %s", msg.SignerAddress)
	}
	signer := RegisteredSigner{SignerId: msg.SignerId, SignerAddress: msg.SignerAddress}
	if err := server.keeper.Signers.Set(ctx, msg.SignerId, signer); err != nil {
		return nil, err
	}
	if err := server.keeper.SignerIdByAddress.Set(ctx, msg.SignerAddress, msg.SignerId); err != nil {
		return nil, err
	}
	emitLockboxEvent(ctx, "signer_registered", sdk.NewAttribute("signer_id", msg.SignerId))
	return &MsgRegisterSignerResponse{}, nil
}

func (server *msgServer) RegisterAccount(ctx context.Context, msg *MsgRegisterAccount) (*MsgRegisterAccountResponse, error) {
	if err := policylogic.ValidateIdentifier("account_id", msg.AccountId); err != nil {
		return nil, invalidField(err)
	}
	if err := policylogic.ValidatePublicKeyHex("registration_key", msg.RegistrationKey); err != nil {
		return nil, invalidField(err)
	}
	if exists, err := server.keeper.Accounts.Has(ctx, msg.AccountId); err != nil {
		return nil, err
	} else if exists {
		return nil, errorsmod.Wrapf(ErrAlreadyExists, "account %s", msg.AccountId)
	}
	account := Account{AccountId: msg.AccountId, RegistrationKey: msg.RegistrationKey}
	if err := server.keeper.Accounts.Set(ctx, msg.AccountId, account); err != nil {
		return nil, err
	}
	emitLockboxEvent(ctx, "account_registered", sdk.NewAttribute("account_id", msg.AccountId))
	return &MsgRegisterAccountResponse{}, nil
}

func (server *msgServer) ClaimOwner(ctx context.Context, msg *MsgClaimOwner) (*MsgClaimOwnerResponse, error) {
	account, err := server.keeper.Accounts.Get(ctx, msg.AccountId)
	if err != nil {
		return nil, errorsmod.Wrapf(ErrNotFound, "account %s", msg.AccountId)
	}
	if account.OwnerAddress != "" {
		return nil, errorsmod.Wrapf(ErrAlreadyExists, "account %s already has an owner", msg.AccountId)
	}
	sdkContext := sdk.UnwrapSDKContext(ctx)
	claim := policylogic.OwnerClaim{
		ChainId:         sdkContext.ChainID(),
		AccountId:       msg.AccountId,
		RegistrationKey: account.RegistrationKey,
		OwnerAddress:    msg.OwnerAddress,
		Nonce:           msg.Nonce,
		IssuedAt:        msg.IssuedAt,
		ExpiresAt:       msg.ExpiresAt,
	}
	if err := policylogic.ValidateOwnerClaimWindow(claim, uint64(sdkContext.BlockTime().Unix())); err != nil {
		return nil, invalidField(err)
	}
	claimText, err := policylogic.OwnerClaimText(claim)
	if err != nil {
		return nil, invalidField(err)
	}
	if err := policylogic.VerifyEip191Owner(claimText, msg.OwnerSignature, msg.OwnerAddress); err != nil {
		return nil, refusedOwnerSignature(err)
	}
	if err := policylogic.VerifyEd25519Signature(account.RegistrationKey, []byte(claimText), msg.RegistrationSignature); err != nil {
		return nil, refusedOwnerSignature(err)
	}
	account.OwnerAddress = msg.OwnerAddress
	if err := server.keeper.Accounts.Set(ctx, msg.AccountId, account); err != nil {
		return nil, err
	}
	emitLockboxEvent(ctx, "owner_claimed", sdk.NewAttribute("account_id", msg.AccountId), sdk.NewAttribute("owner_address", msg.OwnerAddress))
	return &MsgClaimOwnerResponse{}, nil
}

func (server *msgServer) RegisterKeyRef(ctx context.Context, msg *MsgRegisterKeyRef) (*MsgRegisterKeyRefResponse, error) {
	if err := policylogic.ValidateIdentifier("key_ref", msg.KeyRef); err != nil {
		return nil, invalidField(err)
	}
	if err := policylogic.ValidateEthereumAddress("venue_address", msg.VenueAddress); err != nil {
		return nil, invalidField(err)
	}
	if err := policylogic.ValidateCommittee(msg.Committee, msg.Threshold); err != nil {
		return nil, invalidField(err)
	}
	if exists, err := server.keeper.KeyRefs.Has(ctx, msg.KeyRef); err != nil {
		return nil, err
	} else if exists {
		return nil, errorsmod.Wrapf(ErrAlreadyExists, "key_ref %s", msg.KeyRef)
	}
	for _, signerId := range msg.Committee {
		if exists, err := server.keeper.Signers.Has(ctx, signerId); err != nil {
			return nil, err
		} else if !exists {
			return nil, errorsmod.Wrapf(ErrNotFound, "committee member %s is not a registered signer", signerId)
		}
	}
	ownerAddress, err := server.keeper.requireOwnerAddressOfAccount(ctx, msg.AccountId)
	if err != nil {
		return nil, err
	}
	registrationText := policylogic.KeyRefRegistrationText(policylogic.KeyRefRegistration{
		ChainId:      sdk.UnwrapSDKContext(ctx).ChainID(),
		KeyRef:       msg.KeyRef,
		AccountId:    msg.AccountId,
		VenueAddress: msg.VenueAddress,
		Committee:    msg.Committee,
		Threshold:    msg.Threshold,
	})
	if err := policylogic.VerifyEip191Owner(registrationText, msg.OwnerSignature, ownerAddress); err != nil {
		return nil, refusedOwnerSignature(err)
	}
	keyRef := KeyRef{KeyRef: msg.KeyRef, AccountId: msg.AccountId, VenueAddress: msg.VenueAddress, Committee: msg.Committee, Threshold: msg.Threshold}
	if err := server.keeper.KeyRefs.Set(ctx, msg.KeyRef, keyRef); err != nil {
		return nil, err
	}
	emitLockboxEvent(ctx, "key_ref_registered", sdk.NewAttribute("key_ref", msg.KeyRef), sdk.NewAttribute("account_id", msg.AccountId))
	return &MsgRegisterKeyRefResponse{}, nil
}
