import functools
import time
from abc import abstractmethod
from typing import Any
from typing import Dict
from typing import FrozenSet
from typing import Iterable
from typing import List
from typing import Optional
from typing import Set

from util.print import assert_with_trace
from util.print import eprint

from .es_doc import EsDoc
from .event import ConsensusFinalizedEvent
from .event import ControlePlaneSpawnAcceptTaskTlsServerHandshakeFailedEvent
from .event import CupShareProposedEvent
from .event import DeliverBatchEvent
from .event import Event
from .event import FinalEvent
from .event import FinalizedEvent
from .event import GenericLogEvent
from .event import MoveBlockProposalEvent
from .event import NodeMembershipEvent
from .event import OriginallyInSubnetPreambleEvent
from .event import OriginalSubnetTypePreambleEvent
from .event import RebootEvent
from .event import RebootIntentEvent
from .event import RegistryNodeAddedEvent
from .event import RegistryNodeRemovedEvent
from .event import RegistrySubnetCreatedEvent
from .event import RegistrySubnetUpdatedEvent
from .event import ReplicaDivergedEvent
from .event import ValidatedBlockProposalEvent
from .global_infra import GlobalInfra


class Timed:
    @staticmethod
    def default() -> Dict[str, float]:
        return {
            "wall_clock_time_seconds": 0.0,
            "process_time_seconds": 0.0,
            "perf_counter_seconds": 0.0,
        }

    def __init__(self, accumulator: Dict[str, str]):
        self.accumulator = accumulator
        self.wlclock_time_start = None
        self.process_time_start = None
        self.perf_counter_start = None

    def __enter__(self):
        """Starts the timers"""
        self.wlclock_time_start = time.time()
        self.process_time_start = time.process_time()
        self.perf_counter_start = time.perf_counter()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        """Accumulates the elapsed time"""
        self.accumulator["wall_clock_time_seconds"] += time.time() - self.wlclock_time_start

        self.accumulator["process_time_seconds"] += time.process_time() - self.process_time_start

        self.accumulator["perf_counter_seconds"] += time.perf_counter() - self.perf_counter_start


class OutcomeHandler:
    def __init__(self, exit_code: int, should_crash: bool, should_be_violated: bool):
        self.exit_code = exit_code
        self.should_crash = should_crash
        self.should_be_violated = should_be_violated


NORMAL = OutcomeHandler(0, False, False)


class PreProcessor:

    stat: Dict[str, Any]

    def __init__(self, name: str):
        self.name = name
        self.stat = {
            "test_runtime_milliseconds": 0.0,
            "pre_processing": Timed.default(),
        }
        self._elapsed_time = 0.0

    @abstractmethod
    def get_formulas(self) -> Dict[str, OutcomeHandler]:
        """
        Returns a dict of (name: OutcomeHandler) pairs for the MFOTL formulas that
        depend on this pre-processor.

        See https://gitlab.com/ic-monitoring/mfotl-policies
        """
        ...

    @abstractmethod
    def process_log_entry(self, doc: EsDoc) -> Iterable[str]:
        """
        Returns a generator of events (in string representation) corresponding
         to this ES document.
        - Theoretically, each ES document may yield multiple events.
        - Typically, an ES document yields a unique event.
        """
        ...

    def preamble(self) -> Iterable[str]:
        """
        Returns a generator of synthetic preamble events added at the very
        beginning of the testnet run. For example, this is used for passing
        the information about original subnet membership of nodes to Monpoly.
        """
        return []

    def run(self, logs: Iterable[EsDoc]) -> Iterable[str]:
        """Returns a generator of events corresponding to [logs]"""
        eprint(f"Running pre-processor {self.name} ...")

        timestamp = 0
        first_timestamp = None

        # Synthetic events added at the very beginning of the testnet run
        eprint(" Generating preamble relations ...", end=None, flush=True)
        with Timed(self.stat["pre_processing"]):
            yield from self.preamble()
        eprint(" done.", flush=True)

        eprint(" Processing logs ...", end=None, flush=True)

        for doc in logs:
            timestamp = doc.unix_ts()
            if not first_timestamp:
                first_timestamp = timestamp

            for event in self.process_log_entry(doc):
                with Timed(self.stat["pre_processing"]):
                    yield event

        # synthetic event added as the very last event of the testnet run
        with Timed(self.stat["pre_processing"]):
            yield from FinalEvent(timestamp).compile()

        eprint(" done.", flush=True)

        # report test runtime statistics
        if first_timestamp:
            self.stat["test_runtime_milliseconds"] = timestamp - first_timestamp
        else:
            self.stat["test_runtime_milliseconds"] = 0.0

        eprint(f"Pre-processor {self.name} completed.")


class DeclarativePreProcessor(PreProcessor):

    LOG_EVENTS = frozenset(
        [
            "log",
            "reboot",
            "reboot_intent",
            "p2p__node_added",
            "p2p__node_removed",
            "deliver_batch",
            "consensus_finalized",
            "move_block_proposal",
            "ControlPlane__spawn_accept_task__tls_server_handshake_failed",
            "registry__node_added_to_subnet",
            "registry__node_removed_from_subnet",
            "CUP_share_proposed",
            "replica_diverged",
            "finalized",
        ]
    )

    PREAMBLE_EVENTS = frozenset(
        [
            "original_subnet_type",
            "originally_in_subnet",
        ]
    )

    GLOBAL_INFRA_BASED_EVENTS = frozenset(
        [
            "original_subnet_type",
            "originally_in_subnet",
            "registry__node_added_to_subnet",
            "registry__node_removed_from_subnet",
        ]
    )

    class UnknownPredicateError(Exception):
        """Predicate name is unknown"""

        def __init__(self, unknown_pred: str):
            super().__init__(unknown_pred)

    class UnknownPreambleEventNameError(Exception):
        """Preamble event name is unknown"""

        def __init__(self, unknown_pred: str):
            super().__init__(unknown_pred)

    _infra: Optional[GlobalInfra]

    def get_event_stream_builder(self, pred: str, doc: EsDoc) -> Event:
        if pred == "log":
            return GenericLogEvent(doc)
        if pred == "reboot":
            return RebootEvent(doc)
        if pred == "reboot_intent":
            return RebootIntentEvent(doc)
        if pred == "p2p__node_added":
            return NodeMembershipEvent(doc, verb="added")
        if pred == "p2p__node_removed":
            return NodeMembershipEvent(doc, verb="removed")
        if pred == "validated_BlockProposal_Added":
            return ValidatedBlockProposalEvent(doc, verb="Added")
        if pred == "validated_BlockProposal_Moved":
            return ValidatedBlockProposalEvent(doc, verb="Moved")
        if pred == "deliver_batch":
            return DeliverBatchEvent(doc)
        if pred == "consensus_finalized":
            return ConsensusFinalizedEvent(doc)
        if pred == "move_block_proposal":
            return MoveBlockProposalEvent(doc)
        if pred == "ControlPlane__spawn_accept_task__tls_server_handshake_failed":
            return ControlePlaneSpawnAcceptTaskTlsServerHandshakeFailedEvent(doc)
        if pred == "registry__subnet_created":
            return RegistrySubnetCreatedEvent(doc)
        if pred == "registry__subnet_updated":
            return RegistrySubnetUpdatedEvent(doc)
        if pred == "registry__node_added_to_subnet":
            assert self._infra is not None, f"{pred} event requires global infra"
            return RegistryNodeAddedEvent(doc, self._infra)
        if pred == "registry__node_removed_from_subnet":
            assert self._infra is not None, f"{pred} event requires global infra"
            return RegistryNodeRemovedEvent(doc, self._infra)
        if pred == "replica_diverged":
            return ReplicaDivergedEvent(doc)
        if pred == "CUP_share_proposed":
            return CupShareProposedEvent(doc)
        if pred == "finalized":
            return FinalizedEvent(doc)

        raise DeclarativePreProcessor.UnknownPredicateError(pred)

    def preamble_builder(self, pred: str) -> Event:
        if pred == "original_subnet_type":
            assert self._infra is not None, f"{pred} preamble event requires global infra"
            return OriginalSubnetTypePreambleEvent(self._infra)
        if pred == "originally_in_subnet":
            assert self._infra is not None, f"{pred} preamble event requires global infra"
            return OriginallyInSubnetPreambleEvent(self._infra)

        raise DeclarativePreProcessor.UnknownPreambleEventNameError(pred)

    def __init__(
        self,
        name: str,
        required_predicates: FrozenSet[str],
        infra: Optional[GlobalInfra],
        required_preamble_events: FrozenSet[str] = frozenset(),
    ):
        super().__init__(name)
        self.required_predicates = required_predicates
        self.required_preamble_events = required_preamble_events
        self._infra = infra

    def preamble(self) -> Iterable[str]:
        for p_name in self.required_preamble_events:
            event: Event = self.preamble_builder(p_name)
            yield from event.compile()

    def process_log_entry(self, doc: EsDoc) -> Iterable[str]:
        for pred in self.required_predicates:
            event: Event = self.get_event_stream_builder(pred, doc)
            yield from event.compile()


class UniversalPreProcessor(DeclarativePreProcessor):
    """Pre-processor that requires only the events needed for specifiec policies"""

    _POLICIES: Dict[str, Dict[str, FrozenSet[str]]]
    _POLICIES = {
        # Disabled because violations are not actionable
        # "artifact_pool_latency": {
        #     "preambles": frozenset(
        #         [
        #             "original_subnet_type",
        #         ]
        #     ),
        #     "dependencies": frozenset(
        #         [
        #             "p2p__node_added",
        #             "p2p__node_removed",
        #             "registry__subnet_created",
        #             "registry__subnet_updated",
        #             "validated_BlockProposal_Added",
        #             "validated_BlockProposal_Moved",
        #         ]
        #     ),
        # },
        "unauthorized_connections": {
            "preambles": frozenset(
                [
                    "originally_in_subnet",
                ]
            ),
            "dependencies": frozenset(
                [
                    "ControlPlane__spawn_accept_task__tls_server_handshake_failed",
                    "registry__node_added_to_subnet",
                    "registry__node_removed_from_subnet",
                ]
            ),
        },
        "reboot_count": {
            "preambles": frozenset([]),
            "dependencies": frozenset(["reboot", "reboot_intent"]),
        },
        "finalization_consistency": {
            "preambles": frozenset([]),
            "dependencies": frozenset(
                [
                    "finalized",
                ]
            ),
        },
        "finalized_height": {
            "preambles": frozenset([]),
            "dependencies": frozenset(
                [
                    "finalized",
                ]
            ),
        },
        "clean_logs": {
            "preambles": frozenset([]),
            "dependencies": frozenset(["log"]),
        },
    }

    @staticmethod
    def _preambles(policy: str) -> FrozenSet[str]:
        return UniversalPreProcessor._POLICIES[policy]["preambles"]

    @staticmethod
    def _dependencies(policy: str) -> FrozenSet[str]:
        return UniversalPreProcessor._POLICIES[policy]["dependencies"]

    @staticmethod
    def is_global_infra_required(formula_names: Optional[Set[str]]) -> bool:

        if formula_names is None:
            # All formulas are enabled
            return True

        preds = frozenset(
            [
                pred
                for formula in formula_names
                for pred in UniversalPreProcessor._dependencies(formula).union(
                    UniversalPreProcessor._preambles(formula)
                )
            ]
        )
        return len(preds.intersection(DeclarativePreProcessor.GLOBAL_INFRA_BASED_EVENTS)) > 0

    formulas: Dict[str, OutcomeHandler]

    def get_formulas(self) -> Dict[str, OutcomeHandler]:
        return self.formulas

    @staticmethod
    def get_supported_formulas() -> List[str]:
        """Returns list of all the formulas supported by this pre-processor"""
        return sorted(list(UniversalPreProcessor._POLICIES.keys()))

    @staticmethod
    def get_supported_formulas_wo_global_infra() -> List[str]:
        return list(
            filter(
                lambda f: not UniversalPreProcessor.is_global_infra_required(set([f])),
                UniversalPreProcessor.get_supported_formulas(),
            )
        )

    def __init__(self, infra: Optional[GlobalInfra], formulas: Optional[Set[str]] = None):

        if formulas is None:
            formulas = set(UniversalPreProcessor._POLICIES.keys())

        all_formulas = UniversalPreProcessor.get_supported_formulas()
        assert_with_trace(formulas.issubset(all_formulas), "unexpected formulas")

        unit: FrozenSet[str] = frozenset()
        required_preamble_events = functools.reduce(
            lambda val, elem: val.union(elem), [UniversalPreProcessor._preambles(f) for f in formulas], unit
        )
        required_predicates = functools.reduce(
            lambda val, elem: val.union(elem), [UniversalPreProcessor._dependencies(f) for f in formulas], unit
        )

        eprint(f"Creating UniversalPreProcessor supporting formulas: {', '.join(formulas)}")

        super().__init__(
            name="unipol",
            infra=infra,
            required_preamble_events=required_preamble_events,
            required_predicates=required_predicates,
        )

        self.formulas = {f: NORMAL for f in formulas}
