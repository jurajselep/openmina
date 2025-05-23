import { Injectable } from '@angular/core';
import {
  NodesOverviewLedger,
  NodesOverviewLedgerStepState
} from '@shared/types/nodes/dashboard/nodes-overview-ledger.type';
import * as Sentry from '@sentry/angular';
import {
  NodesOverviewBlock,
  NodesOverviewNodeBlockStatus
} from '@shared/types/nodes/dashboard/nodes-overview-block.type';
import { lastItem, ONE_BILLION } from '@openmina/shared';
import { getElapsedTime } from '@shared/helpers/date.helper';
import {
  BlockProductionWonSlotsSlot
} from '@shared/types/block-production/won-slots/block-production-won-slots-slot.type';
import { BlockProductionAttempt } from '@app/app.service';

@Injectable({
  providedIn: 'root',
})
export class SentryService {

  private ledgerIsSynced: boolean = false;
  private blockIsSynced: boolean = false;
  private ledgerSyncedTime: number;
  private blockSyncedTime: number;

  updateLedgerSyncStatus(ledger: NodesOverviewLedger, publicKey: string): void {
    if (this.ledgerIsSynced) {
      return;
    }
    if (ledger.rootStaged.state === NodesOverviewLedgerStepState.SUCCESS) {
      this.ledgerIsSynced = true;
      const syncDetails = {
        stakingLedger: {
          fetchHashes: ledger.stakingEpoch.snarked.fetchHashesDuration + 's',
          fetchAccounts: ledger.stakingEpoch.snarked.fetchAccountsDuration + 's',
        },
        nextEpochLedger: {
          fetchHashes: ledger.nextEpoch.snarked.fetchHashesDuration + 's',
          fetchAccounts: ledger.nextEpoch.snarked.fetchAccountsDuration + 's',
        },
        snarkedRootLedger: {
          fetchHashes: ledger.rootSnarked.snarked.fetchHashesDuration + 's',
          fetchAccounts: ledger.rootSnarked.snarked.fetchAccountsDuration + 's',
        },
        stagedRootLedger: {
          fetchParts: ledger.rootStaged.staged.fetchPartsDuration + 's',
          reconstruct: ledger.rootStaged.staged.reconstructDuration + 's',
        },
      };

      const syncedIn = Math.round((ledger.rootStaged.staged.reconstructEnd - ledger.stakingEpoch.snarked.fetchHashesStart) / ONE_BILLION);
      this.ledgerSyncedTime = syncedIn;

      Sentry.captureMessage(`Ledger synced in ${getElapsedTime(syncedIn)}`, {
        level: 'info',
        tags: {
          type: 'webnode', subType: 'sync.ledger', publicKey, duration: syncedIn
        },
        contexts: { ledger: syncDetails },
        fingerprint: this.fingerprint,
      });
    }
  }

  updateBlockSyncStatus(blocks: NodesOverviewBlock[], startTime: number, publicKey: string): void {
    if (this.blockIsSynced) {
      return;
    }

    const blocksSynced = blocks.every(b => b.status === NodesOverviewNodeBlockStatus.APPLIED);
    if (blocksSynced && blocks[0]) {
      this.blockIsSynced = true;
      blocks = blocks.slice(1);
      const bestTipBlock = blocks[0].height;
      const root = lastItem(blocks).height;
      this.blockSyncedTime = Math.round((Date.now() - startTime) / 1000);
      Sentry.captureMessage(`Last 290 blocks synced in ${getElapsedTime(this.blockSyncedTime)}`, {
        level: 'info',
        tags: {
          type: 'webnode', subType: 'sync.block', publicKey, duration: this.blockSyncedTime
        },
        contexts: { blocks: { bestTipBlock, root } },
        fingerprint: this.fingerprint,
      });

      const syncTotal = this.ledgerSyncedTime + this.blockSyncedTime;
      setTimeout(() => {
        Sentry.captureMessage(`Web Node Synced in ${getElapsedTime(syncTotal)}`, {
          level: 'info',
          tags: {
            type: 'webnode', subType: 'sync.total', publicKey, duration: syncTotal
          },
          fingerprint: this.fingerprint,
        });
      }, 2000);
    }
  }

  updatePeersConnected(seconds: number, publicKey: string): void {
    Sentry.captureMessage(`Web Node connected in ${seconds.toFixed(1)}s`, {
      level: 'info',
      tags: { type: 'webnode', subType: 'sync.peers', publicKey, duration: seconds },
      fingerprint: this.fingerprint,
    });
  }

  updateProducedBlock(attempt: BlockProductionAttempt, publicKey: string): void {
    const times = {
      stagedLedgerDiffCreate: !attempt.times.staged_ledger_diff_create_end || !attempt.times.staged_ledger_diff_create_start
        ? 0 : (attempt.times.staged_ledger_diff_create_end - attempt.times.staged_ledger_diff_create_start) / ONE_BILLION,
      produced: !attempt.times.produced || !attempt.times.staged_ledger_diff_create_end
        ? 0 : (attempt.times.produced - attempt.times.staged_ledger_diff_create_end) / ONE_BILLION,
      proofCreate: !attempt.times.proof_create_end || !attempt.times.proof_create_start
        ? 0 : (attempt.times.proof_create_end - attempt.times.proof_create_start) / ONE_BILLION,
    };

    Sentry.captureMessage(`Block Production Finished (${attempt.status}) - ` + attempt.block?.height, {
      level: 'info',
      tags: { type: 'webnode', subType: 'block.production', publicKey, duration: times.stagedLedgerDiffCreate + times.produced + times.proofCreate },
      fingerprint: this.fingerprint,
      contexts: { block: this.flattenObject(attempt) },
    });
  }

  updateHeartbeat(data: any, publicKey: string): void {
    Sentry.captureMessage('Heartbeat', {
      level: 'info',
      tags: { type: 'webnode', subType: 'heartbeat', publicKey },
      contexts: { heartbeat: { payload: data.payload, signatureField: data.signature.field, signatureScalar: data.signature.scalar } },
      fingerprint: this.fingerprint,
    });
  }

  private flattenObject(obj: Record<string, any>, prefix: string = ''): Record<string, any> {
    return Object.keys(obj).reduce((acc: Record<string, any>, key: string) => {
      const prefixedKey = prefix ? `${prefix}.${key}` : key;

      if (typeof obj[key] === 'object' && obj[key] !== null && !Array.isArray(obj[key])) {
        const nestedObj = this.flattenObject(obj[key], prefixedKey);
        Object.assign(acc, nestedObj);
      } else {
        acc[prefixedKey] = obj[key];
      }

      return acc;
    }, {});
  }

  private get fingerprint(): string[] {
    return [Math.random().toString(36).substring(2, 22)];
  }
}
