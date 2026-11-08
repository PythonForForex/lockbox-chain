package lockbox

import (
	"context"
	"fmt"

	errorsmod "cosmossdk.io/errors"

	sdk "github.com/cosmos/cosmos-sdk/types"

	"lockbox/chain/policylogic"
)

func (server *msgServer) AllowRelease(ctx context.Context, msg *MsgAllowRelease) (*MsgAllowReleaseResponse, error) {
	if err := server.keeper.requireValidatorAdmin(ctx, msg.Submitter); err != nil {
		return nil, err
	}
	if err := policylogic.ValidateHashHex("binary_hash", msg.BinaryHash); err != nil {
		return nil, invalidField(err)
	}
	if exists, err := server.keeper.AllowedReleases.Has(ctx, msg.BinaryHash); err != nil {
		return nil, err
	} else if exists {
		return nil, errorsmod.Wrapf(ErrAlreadyExists, "release %s", msg.BinaryHash)
	}
	delayBlocks, err := server.keeper.ReleaseActivationDelayBlocks.Get(ctx)
	if err != nil {
		return nil, err
	}
	activateHeight := uint64(sdk.UnwrapSDKContext(ctx).BlockHeight()) + delayBlocks
	release := AllowedRelease{BinaryHash: msg.BinaryHash, ActivateHeight: activateHeight}
	if err := server.keeper.AllowedReleases.Set(ctx, msg.BinaryHash, release); err != nil {
		return nil, err
	}
	emitLockboxEvent(ctx, "release_allowed", sdk.NewAttribute("binary_hash", msg.BinaryHash), sdk.NewAttribute("activate_height", fmt.Sprintf("%d", activateHeight)))
	return &MsgAllowReleaseResponse{ActivateHeight: activateHeight}, nil
}

func (server *msgServer) RecordDecision(ctx context.Context, msg *MsgRecordDecision) (*MsgRecordDecisionResponse, error) {
	if err := policylogic.ValidateDecisionFields(msg.RequestId, msg.KeyRef, msg.SignerId, msg.Decision, msg.Reason, msg.IntentHash); err != nil {
		return nil, invalidField(err)
	}
	registeredSignerId, err := server.keeper.SignerIdByAddress.Get(ctx, msg.Submitter)
	if err != nil {
		return nil, errorsmod.Wrapf(ErrUnauthorized, "%s is not a registered signer", msg.Submitter)
	}
	if registeredSignerId != msg.SignerId {
		return nil, errorsmod.Wrapf(ErrUnauthorized, "%s is registered as %s, not %s", msg.Submitter, registeredSignerId, msg.SignerId)
	}
	sequence, err := server.keeper.NextLogSequence.Next(ctx)
	if err != nil {
		return nil, err
	}
	entry := LogEntry{
		Height:     uint64(sdk.UnwrapSDKContext(ctx).BlockHeight()),
		RequestId:  msg.RequestId,
		KeyRef:     msg.KeyRef,
		SignerId:   msg.SignerId,
		Decision:   msg.Decision,
		Reason:     msg.Reason,
		IntentHash: msg.IntentHash,
	}
	if err := server.keeper.LogEntries.Set(ctx, sequence, entry); err != nil {
		return nil, err
	}
	emitLockboxEvent(ctx, "decision_recorded", sdk.NewAttribute("request_id", msg.RequestId), sdk.NewAttribute("signer_id", msg.SignerId), sdk.NewAttribute("decision", msg.Decision))
	return &MsgRecordDecisionResponse{}, nil
}

func (server *msgServer) AnchorTape(ctx context.Context, msg *MsgAnchorTape) (*MsgAnchorTapeResponse, error) {
	if err := policylogic.ValidateIdentifier("segment_id", msg.SegmentId); err != nil {
		return nil, invalidField(err)
	}
	if err := policylogic.ValidateHashHex("segment_hash", msg.SegmentHash); err != nil {
		return nil, invalidField(err)
	}
	isRegisteredSigner, err := server.keeper.SignerIdByAddress.Has(ctx, msg.Submitter)
	if err != nil {
		return nil, err
	}
	if !isRegisteredSigner {
		if err := server.keeper.requireValidatorAdmin(ctx, msg.Submitter); err != nil {
			return nil, err
		}
	}
	if exists, err := server.keeper.TapeAnchors.Has(ctx, msg.SegmentId); err != nil {
		return nil, err
	} else if exists {
		return nil, errorsmod.Wrapf(ErrAlreadyExists, "segment %s", msg.SegmentId)
	}
	anchor := TapeAnchor{SegmentId: msg.SegmentId, SegmentHash: msg.SegmentHash}
	if err := server.keeper.TapeAnchors.Set(ctx, msg.SegmentId, anchor); err != nil {
		return nil, err
	}
	emitLockboxEvent(ctx, "tape_anchored", sdk.NewAttribute("segment_id", msg.SegmentId), sdk.NewAttribute("segment_hash", msg.SegmentHash))
	return &MsgAnchorTapeResponse{}, nil
}
