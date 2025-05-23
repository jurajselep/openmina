import { ChangeDetectionStrategy, Component, OnDestroy, OnInit, TemplateRef, ViewChild, ViewContainerRef } from '@angular/core';
import { StoreDispatcher } from '@shared/base-classes/store-dispatcher.class';
import { BlockProductionWonSlotsSelectors } from '@block-production/won-slots/block-production-won-slots.state';
import {
  BlockProductionWonSlotsSlot,
  BlockProductionWonSlotsStatus,
  BlockProductionWonSlotTimes,
} from '@shared/types/block-production/won-slots/block-production-won-slots-slot.type';
import { getTimeDiff } from '@shared/helpers/date.helper';
import { any, hasValue, isDesktop, isMobile, noMillisFormat, ONE_THOUSAND, safelyExecuteInBrowser, SecDurationConfig, toReadableDate } from '@openmina/shared';
import { filter } from 'rxjs';
import { BlockProductionWonSlotsActions } from '@block-production/won-slots/block-production-won-slots.actions';
import { AppSelectors } from '@app/app.state';
import { AppNodeDetails } from '@shared/types/app/app-node-details.type';
import { Router } from '@angular/router';
import { Routes } from '@shared/enums/routes.enum';

@Component({
  selector: 'mina-block-production-won-slots-side-panel',
  templateUrl: './block-production-won-slots-side-panel.component.html',
  styleUrls: ['./block-production-won-slots-side-panel.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
  host: { class: 'flex-column h-100' },
})
export class BlockProductionWonSlotsSidePanelComponent extends StoreDispatcher implements OnInit, OnDestroy {

  protected readonly BlockProductionWonSlotsStatus = BlockProductionWonSlotsStatus;
  protected readonly config: SecDurationConfig = {
    color: false,
    includeMinutes: true,
    undefinedAlternative: undefined,
    valueIsZeroFn: () => '<1ms',
  };
  protected readonly noMillisFormat = noMillisFormat;
  isMobile = isMobile();
  title: string;

  slot: BlockProductionWonSlotsSlot;
  remainingTime: string;
  scheduled: string;
  slotStartedAlready: boolean;

  vrfText: string;
  vrf: [number, number] = [0, 0];
  discardedOpen: boolean = true;
  percentage: number;
  private timer: any;
  private stopTimer: boolean;
  private stateWhenReachedZero: { globalSlot: number; status: BlockProductionWonSlotsStatus };
  private network: string;

  @ViewChild('beforeLedger', { read: ViewContainerRef }) private beforeLedger: ViewContainerRef;
  @ViewChild('ledger', { read: ViewContainerRef }) private ledger: ViewContainerRef;
  @ViewChild('produced', { read: ViewContainerRef }) private produced: ViewContainerRef;
  @ViewChild('proof', { read: ViewContainerRef }) private proof: ViewContainerRef;
  @ViewChild('apply', { read: ViewContainerRef }) private apply: ViewContainerRef;

  @ViewChild('discarded') private discardedTemplate: TemplateRef<void>;

  constructor(private router: Router) {super();}

  ngOnInit(): void {
    this.listenToActiveSlot();
    this.parseRemainingTime();
    this.listenToActiveNode();
  }

  private listenToActiveNode(): void {
    this.select(AppSelectors.activeNodeDetails, (node: AppNodeDetails) => {
      this.network = node.network?.toLowerCase();
    }, filter(Boolean));
  }

  private listenToActiveSlot(): void {
    this.select(BlockProductionWonSlotsSelectors.activeSlot, (slot: BlockProductionWonSlotsSlot) => {
      this.slot = slot;
      this.title = slot.message;
      this.percentage = [
        slot.times?.stagedLedgerDiffCreate,
        slot.times?.produced,
        slot.times?.proofCreate,
        slot.times?.blockApply,
        slot.times?.committed,
      ].filter(t => hasValue(t)).length * 20;

      this.scheduled = toReadableDate(slot.slotTime);
      this.slotStartedAlready = slot.slotTime < Date.now();

      if (
        (this.stateWhenReachedZero?.globalSlot === slot.globalSlot && this.stateWhenReachedZero?.status !== slot.status)
        || !this.stateWhenReachedZero
      ) {
        this.stopTimer = !this.slot.active;
        this.stateWhenReachedZero = undefined;
      }

      this.parse();

      this.vrfText = this.getVrfText;
      this.vrf = slot.vrfValueWithThreshold;

      this.createDiscardedView();

      this.detect();
    }, filter(Boolean));
  }

  viewInMinascan(): void {
    const url = `https://minascan.io/${this.network}/block/${this.slot.hash}`;
    safelyExecuteInBrowser(() => window.open(url, '_blank'));
  }

  private get getVrfText(): string {
    if (this.slot.active) {
      return 'Won Slot Requirement';
    }
    return 'Block Produce Right';
  }

  private parseRemainingTime(): void {
    this.timer = setInterval(() => {
      this.parse();
    }, 1000);
  }

  private parse(): void {
    if (this.slot && !this.stopTimer) {
      const remainingTime = getTimeDiff(this.addMinutesToTimestamp(this.slot.slotTime, 3), { withSecs: true });
      if (remainingTime.inFuture) {
        this.remainingTime = '-';
      }
      this.remainingTime = remainingTime.diff;
      if (this.remainingTime === '0s') {
        /* when we reached 0s, we need to fetch data again because this slot is over and the user should see that in the table */
        this.stopTimer = true;
        this.stateWhenReachedZero = { globalSlot: this.slot.globalSlot, status: this.slot.status };
        this.remainingTime = '-';
      }
      this.detect();
    } else {
      this.remainingTime = '-';
    }
  }

  private addMinutesToTimestamp(timestampInMilliseconds: number, minutesToAdd: number): number {
    return timestampInMilliseconds + minutesToAdd * ONE_THOUSAND * 60;
  }

  private createDiscardedView(): void {
    this.beforeLedger?.clear();
    this.apply?.clear();
    this.proof?.clear();
    this.produced?.clear();
    this.ledger?.clear();

    if (this.slot.discardReason) {
      const times: BlockProductionWonSlotTimes = this.slot.times;
      let locationName: string;
      if (times.discarded < times.stagedLedgerDiffCreateEnd) {
        locationName = 'beforeLedger';
      } else if (times.discarded >= times.stagedLedgerDiffCreateEnd) {
        locationName = 'ledger';
      }
      if (times.discarded >= times.producedEnd) {
        locationName = 'produced';
      }
      if (times.discarded >= times.proofCreateEnd) {
        locationName = 'proof';
      }
      if (times.discarded >= times.blockApplyEnd) {
        locationName = 'apply';
      }
      (any(this)[locationName] as ViewContainerRef)?.createEmbeddedView(this.discardedTemplate);
    }
  }

  closeSidePanel(): void {
    this.router.navigate([Routes.BLOCK_PRODUCTION, Routes.WON_SLOTS]);
    this.dispatch2(BlockProductionWonSlotsActions.toggleSidePanel());
  }

  override ngOnDestroy(): void {
    super.ngOnDestroy();
    clearInterval(this.timer);
  }

}
