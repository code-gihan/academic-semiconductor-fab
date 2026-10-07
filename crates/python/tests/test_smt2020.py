"""The smt2020 module on the bundled DS1: python -m unittest discover -s crates/python/tests"""

import math
import unittest
from concurrent.futures import ThreadPoolExecutor

import smt2020
from smt2020 import DAY, HOUR

STATES = ("down", "pm", "setup", "process", "load", "unload", "idle")


class SimulationTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dataset = smt2020.load_dataset("ds1")
        # Ten days of releases, then the drain of the lots released.
        cls.config = {"horizon": 10 * DAY}
        straight = smt2020.Simulation(cls.dataset, cls.config)
        cls.finished = straight.run()
        cls.results = straight.results()

    def test_a_run_ends_when_every_released_lot_is_complete(self):
        self.assertTrue(self.finished["finished"])
        self.assertEqual(self.finished["released"], self.finished["completed"])
        self.assertEqual(self.finished["wip"], 0)
        self.assertEqual(self.results["periods"][-1]["name"], "Drain")

    def test_a_paused_run_shows_the_fab_and_resumes_to_the_same_results(self):
        simulation = smt2020.Simulation(self.dataset, self.config)
        progress = simulation.run(until=2 * DAY + 0.5 * HOUR)
        self.assertEqual(progress["now"], 2 * DAY + HOUR // 2)
        self.assertFalse(progress["finished"])
        lots, tools, groups = simulation.lots(), simulation.tools(), simulation.tool_groups()
        self.assertEqual(len(lots), progress["wip"])
        self.assertEqual([lot["id"] for lot in lots], sorted(lot["id"] for lot in lots))
        self.assertEqual(sum(group["tools"] for group in groups), len(tools))
        self.assertEqual(
            sum(group["queue"] for group in groups),
            sum(lot["state"] == "queued" for lot in lots),
        )
        processing = {lot["id"]: lot["tool"] for lot in lots if lot["state"] == "processing"}
        self.assertEqual(processing, {lot: tool["id"] for tool in tools for lot in tool["lots"]})
        for group in groups:
            self.assertEqual(sum(group[state] for state in STATES), group["tools"])
        with self.assertRaises(RuntimeError):
            simulation.results()

        days = []

        def on_progress(progress):
            days.append(progress["now"] // DAY)
            return progress["now"] < 5 * DAY

        self.assertEqual(simulation.run(on_progress=on_progress)["now"], 5 * DAY)
        self.assertEqual(days, [3, 4, 5])
        self.assertEqual(simulation.run(), self.finished)
        self.assertEqual(smt2020.digest(simulation.results()), smt2020.digest(self.results))

    def test_segments_show_the_lots_on_their_clocks_and_their_completions_so_far(self):
        info = self.dataset.info()
        simulation = smt2020.Simulation(self.dataset, self.config)
        progress = simulation.run(until=2 * DAY + 0.5 * HOUR)
        segments = simulation.segments()
        self.assertEqual(len(segments), len(info["segments"]))
        # Every lot in a segment until its exit step starts, with its deadline.
        waiting = {
            lot["id"]: lot
            for lot in simulation.lots()
            if lot["cqt_exit"] is not None
            and not (lot["step"] == lot["cqt_exit"] and lot["state"] == "processing")
        }
        in_segments = [
            (lot, info["segments"][index])
            for index, segment in enumerate(segments)
            for lot in segment["lots"]
        ]
        self.assertTrue(in_segments)
        self.assertEqual(len(in_segments), len(waiting))
        for lot, segment in in_segments:
            status = waiting[lot["id"]]
            self.assertEqual(
                (lot["kind"], lot["step"], lot["state"]),
                (status["kind"], status["step"], status["state"]),
            )
            self.assertEqual(status["cqt_exit"], segment["exit"])
            self.assertEqual(lot["entered"] + segment["limit"], status["cqt_deadline"])
            self.assertLessEqual(lot["slack"], status["cqt_deadline"] - progress["now"])
        for count, total in (("completed", "cqt_completed"), ("violated", "cqt_violated")):
            self.assertEqual(sum(segment["cqt"][count] for segment in segments), progress[total])

    def test_an_exception_of_the_observer_pauses_the_run(self):
        simulation = smt2020.Simulation(self.dataset, self.config)

        def stop(progress):
            raise KeyError(progress["now"])

        with self.assertRaises(KeyError):
            simulation.run(on_progress=stop)
        self.assertEqual(simulation.progress()["now"], DAY)
        simulation.run()
        self.assertEqual(smt2020.digest(simulation.results()), smt2020.digest(self.results))

    def test_reset_starts_over(self):
        simulation = smt2020.Simulation(self.dataset, self.config)
        simulation.run(until=DAY)
        simulation.reset()
        self.assertEqual((simulation.progress()["now"], simulation.progress()["released"]), (0, 0))
        stopping = {"limits": {"LithoTrack_FE_95": {"front": 50, "total": 85}}}
        simulation.reset({**self.config, "queue_time": "qtcr", "stopping": stopping})
        self.assertEqual(simulation.config()["queue_time"], "qtcr")
        self.assertEqual(simulation.config()["seed"], 1)
        # A rejected configuration leaves the simulation as it was.
        with self.assertRaises(ValueError):
            simulation.reset({"horizon": 0})
        self.assertEqual(simulation.config()["queue_time"], "qtcr")

    def test_simulations_run_in_parallel_threads(self):
        def replicate(replication):
            config = {**self.config, "replication": replication}
            simulation = smt2020.Simulation(self.dataset, config)
            simulation.run()
            return smt2020.digest(simulation.results())

        with ThreadPoolExecutor(2) as pool:
            digests = list(pool.map(replicate, [0, 1]))
        self.assertEqual(digests[0], smt2020.digest(self.results))
        self.assertNotEqual(digests[0], digests[1])

    def test_the_dataset_info_names_and_indexes_the_dataset(self):
        info = self.dataset.info()
        self.assertEqual((len(info["tool_groups"]), len(info["segments"])), (106, 66))
        for segment in info["segments"]:
            steps = info["routes"][segment["route"]]["steps"]
            self.assertTrue(segment["entry"] < segment["exit"] < len(steps))
            self.assertIn(steps[segment["exit"]]["tool_group"], segment["tool_groups"])
        self.assertEqual(
            sorted(group["name"] for group in info["tool_groups"] if group["stepper"]),
            ["LithoTrack_FE_115", "LithoTrack_FE_95"],
        )
        self.assertEqual([period["name"] for period in info["periods"][:2]], ["WarmUp", "Period_1"])

    def test_rankings_batch_starts_and_a_warm_up_configure_a_run(self):
        info = self.dataset.info()
        segment = info["segments"][0]
        exit = info["routes"][segment["route"]]["steps"][segment["exit"]]["tool_group"]
        ranking = {info["tool_groups"][exit]["name"]: [{"qt_within": HOUR}, "priority", "fifo"]}
        strategy = {**self.config, "warm_up": 2 * DAY, "batch_start_within": HOUR, "ranking": ranking}
        simulation = smt2020.Simulation(self.dataset, strategy)
        self.assertEqual(simulation.config()["ranking"], ranking)
        simulation.run()
        periods = [period["name"] for period in simulation.results()["periods"]]
        self.assertEqual(periods, ["WarmUp", "Period_1", "Drain"])

    def test_recording_leaves_the_results_unchanged_and_accounts_for_them(self):
        info = self.dataset.info()
        first_group = info["tool_groups"][0]["name"]
        recording = {
            "violations": True,
            "tool_groups": True,
            "events": {"from": DAY, "until": 2 * DAY, "tool_groups": [first_group]},
        }
        simulation = smt2020.Simulation(self.dataset, self.config, recording)
        progress = simulation.run()
        recorded = simulation.results()
        self.assertEqual(smt2020.digest(recorded), smt2020.digest(self.results))
        violated = sum(
            period["cqt_litho"]["violated"] + period["cqt_rest"]["violated"]
            for period in recorded["periods"]
        )
        self.assertEqual(progress["cqt_violated"], violated)
        records = simulation.records()
        self.assertEqual(len(records["violations"]["lot"]), violated)
        for table in records.values():
            self.assertEqual(len({len(column) for column in table.values()}), 1)
        days = records["tool_groups"]
        self.assertEqual(len(days["day"]), len(recorded["days"]) * len(info["tool_groups"]))
        events = records["events"]
        self.assertTrue(all(DAY <= time < 2 * DAY for time in events["time"]))
        self.assertTrue(all(group == 0 for group in events["tool_group"]))
        self.assertTrue(
            all((part is None) == (lot is None) for part, lot in zip(events["part"], events["lot"]))
        )
        self.assertEqual(sum(day["started"] for day in recorded["days"]), recorded["released"])
        self.assertEqual(len(recorded["periods"][0]["cqt_segments"]), len(info["segments"]))
        self.assertIsNone(simulation.flow_factors())
        wip = next(
            row
            for row in smt2020.daily([recorded, self.results])
            if (row["day"], row["scope"], row["measure"]) == (1, "fab", "wip")
        )
        self.assertEqual((wip["n"], wip["std"]), (2, 0.0))

    def test_summaries_and_csv(self):
        summaries = smt2020.summarize([self.results, self.results])
        completed = next(
            row
            for row in summaries
            if (row["period"], row["scope"], row["measure"]) == ("WarmUp", "fab", "completed")
        )
        self.assertEqual((completed["n"], completed["std"]), (2, 0.0))
        self.assertTrue(
            smt2020.csv(summaries).startswith("period,scope,item,kind,measure,n,mean,std,ci95\n")
        )

    def test_comparisons_pair_replications(self):
        def run(replication, queue_time):
            config = {"horizon": 3 * DAY, "queue_time": queue_time, "replication": replication}
            simulation = smt2020.Simulation(self.dataset, config)
            simulation.run()
            return simulation.results()

        baseline = [run(0, "none"), run(1, "none")]
        other = [run(1, "qtcr"), run(0, "qtcr")]
        rows = smt2020.compare(baseline, other)
        vl = next(
            row
            for row in rows
            if (row["period"], row["scope"], row["item"], row["measure"])
            == ("WarmUp", "cqt", "total", "vl_pct")
        )
        self.assertEqual(vl["n"], 2)
        self.assertAlmostEqual(vl["other"] - vl["baseline"], vl["difference"])
        self.assertTrue(all(row["difference"] == 0 for row in smt2020.compare(baseline, baseline)))
        self.assertTrue(
            smt2020.comparison_csv(rows).startswith(
                "period,scope,item,kind,measure,n,baseline,other,difference,std,ci95\n"
            )
        )
        with self.assertRaises(ValueError):
            smt2020.compare(baseline, other[1:])

    def test_bad_input_is_rejected(self):
        with self.assertRaises(ValueError):
            smt2020.Simulation(self.dataset, {"horizon": DAY, "sead": 2})
        unknown_group = {
            "horizon": DAY,
            "queue_time": "qts",
            "stopping": {"limits": {"Nope": {"front": 1, "total": 1}}},
        }
        with self.assertRaises(ValueError):
            smt2020.Simulation(self.dataset, unknown_group)
        with self.assertRaises(ValueError):
            smt2020.Simulation(self.dataset, self.config).run(until=float("nan"))
        with self.assertRaises(ValueError):
            smt2020.Dataset(b"not a dataset")
        with self.assertRaises(OSError):
            smt2020.load_dataset("no/such/file.bin")
        for criteria in (["fifoo"], ["fifo"]):
            with self.assertRaises(ValueError):
                smt2020.Simulation(self.dataset, {"horizon": DAY, "ranking": {"Nope": criteria}})
        with self.assertRaises(ValueError):
            smt2020.Simulation(self.dataset, {"horizon": DAY, "warm_up": DAY})
        for recording in ({"violation": True}, {"events": {"from": DAY, "until": DAY}}):
            with self.assertRaises(ValueError):
                smt2020.Simulation(self.dataset, self.config, recording)

    def state_after(self, simulation, days):
        """Progress, lots, tools and CQT segments of `simulation` after `days`."""
        simulation.run(until=days * DAY)
        return (simulation.progress(), simulation.lots(), simulation.tools(), simulation.segments())

    def test_strategy_code_runs_the_papers_rules_as_the_built_in_rules_do(self):
        info = self.dataset.info()
        horizon = {"horizon": 30 * DAY}

        # The same priority everywhere ranks nothing; the views carry the lot.
        class Constant:
            seen = None

            def priority(self, lot, now):
                if self.seen is None:
                    self.seen = (lot.kind, lot.tool_group, lot.step_name, now, lot.cqt)
                return 0

        constant = Constant()
        code_rule = smt2020.Simulation(self.dataset, {**horizon, "queue_time": "code"}, code=constant)
        self.assertEqual(
            self.state_after(code_rule, 5),
            self.state_after(smt2020.Simulation(self.dataset, horizon), 5),
        )
        kind, group, step, now, _ = constant.seen
        self.assertIsInstance(kind, str)
        self.assertIsInstance(group, str)
        self.assertIsInstance(step, str)
        self.assertIsInstance(now, int)

        # The end of the lot's segment, as code, ranks as the built-in criterion.
        exits = {
            info["tool_groups"][info["routes"][s["route"]]["steps"][s["exit"]]["tool_group"]]["name"]
            for s in info["segments"]
        }

        def ranking(criterion):
            return {**horizon, "ranking": {name: [criterion, "fifo"] for name in exits}}

        class Deadline:
            def priority(self, lot, now):
                return lot.cqt.deadline if lot.cqt else float("inf")

        self.assertEqual(
            self.state_after(smt2020.Simulation(self.dataset, ranking("code"), code=Deadline()), 5),
            self.state_after(smt2020.Simulation(self.dataset, ranking("qt_deadline")), 5),
        )

        # Stopping at the steppers, as admission code.
        steppers = ("LithoTrack_FE_95", "LithoTrack_FE_115")
        stopping = {"limits": {name: {"front": 5, "total": 10} for name in steppers}}

        class Admission:
            held = 0

            def admit(self, lot, segment, now):
                reached = any(
                    group.front >= (5 if group.tool_group in steppers else 1000)
                    or group.total >= (10 if group.tool_group in steppers else 1000)
                    for group in segment.groups
                )
                self.held += reached
                return not reached

        admission = Admission()
        self.assertEqual(
            self.state_after(smt2020.Simulation(self.dataset, horizon, code=admission), 10),
            self.state_after(smt2020.Simulation(self.dataset, {**horizon, "stopping": stopping}), 10),
        )
        self.assertGreater(admission.held, 0)

        # Batches below their minimum start at an hour of slack, as code.
        class Batches:
            started = 0

            def start_batch(self, batch, now):
                if batch.slack is None:
                    return False
                if batch.slack <= HOUR:
                    self.started += 1
                    return True
                return math.ceil(now + batch.slack - HOUR)

        batches = Batches()
        self.assertEqual(
            self.state_after(smt2020.Simulation(self.dataset, horizon, code=batches), 10),
            self.state_after(
                smt2020.Simulation(self.dataset, {**horizon, "batch_start_within": HOUR}), 10
            ),
        )
        self.assertGreater(batches.started, 0)

    def test_strategy_code_errors_stop_the_run(self):
        ranked = {"horizon": 30 * DAY, "queue_time": "code"}

        class Failing:
            def priority(self, lot, now):
                return lot.cqt.slack

        failing = smt2020.Simulation(self.dataset, ranked, code=Failing())
        # The code's exception, then the run's failure.
        with self.assertRaises(AttributeError):
            failing.run()
        with self.assertRaisesRegex(RuntimeError, "strategy code: priority: AttributeError"):
            failing.run()

        class Text:
            def priority(self, lot, now):
                return "soon"

        with self.assertRaisesRegex(RuntimeError, "priority: returned str, not a number"):
            smt2020.Simulation(self.dataset, ranked, code=Text()).run()
        with self.assertRaisesRegex(RuntimeError, "no strategy code was given"):
            smt2020.Simulation(self.dataset, ranked).run(until=DAY)

        class AdmitOnly:
            def admit(self, lot, segment, now):
                return True

        with self.assertRaisesRegex(ValueError, "has no priority"):
            smt2020.Simulation(self.dataset, ranked, code=AdmitOnly())
        with self.assertRaisesRegex(ValueError, "defines no hook"):
            smt2020.Simulation(self.dataset, ranked, code=object())

    def test_ctrl_c_in_the_strategy_code_pauses_the_run(self):
        config = {"horizon": 3 * DAY, "queue_time": "code"}

        class Wafers:
            calls = 0
            interrupt_at = None

            def priority(self, lot, now):
                self.calls += 1
                if self.calls == self.interrupt_at:
                    raise KeyboardInterrupt
                return -lot.wafers

        interrupted = Wafers()
        interrupted.interrupt_at = 100
        simulation = smt2020.Simulation(self.dataset, config, code=interrupted)
        with self.assertRaises(KeyboardInterrupt):
            simulation.run()
        self.assertFalse(simulation.progress()["finished"])
        simulation.run()
        straight = smt2020.Simulation(self.dataset, config, code=Wafers())
        straight.run()
        self.assertEqual(
            smt2020.digest(simulation.results()), smt2020.digest(straight.results())
        )


class AmhsTest(unittest.TestCase):
    def test_the_smat2022_layout_and_amhs_show_where_vehicles_and_foups_are(self):
        dataset = smt2020.load_dataset("smat2022")
        layout = dataset.layout()
        self.assertEqual((len(layout["rails"]), len(layout["ports"])), (3424, 22120))
        self.assertIsNone(smt2020.load_dataset("ds1").layout())
        simulation = smt2020.Simulation(dataset, {"horizon": 730 * DAY, "amhs": {"vehicles": 300}})
        simulation.run(until=HOUR)
        amhs = simulation.amhs()
        self.assertEqual(len(amhs["vehicles"]), 300)
        tools = sum(group["tools"] for group in dataset.info()["tool_groups"])
        self.assertEqual(len(amhs["tools"]), tools)
        lots = simulation.lots()
        carried = sorted(vehicle["lot"] for vehicle in amhs["vehicles"] if vehicle["lot"] is not None)
        self.assertEqual(carried, [lot["id"] for lot in lots if lot["vehicle"] is not None])
        at_ports = {lot["id"] for lot in lots if lot["port"] is not None}
        self.assertTrue({foup["lot"] for foup in amhs["foups"]} <= at_ports)
        ds1 = smt2020.load_dataset("ds1")
        self.assertIsNone(smt2020.Simulation(ds1, {"horizon": DAY}).amhs())
        with self.assertRaisesRegex(ValueError, "no AMHS layout"):
            smt2020.Simulation(ds1, {"horizon": DAY, "amhs": {}})

    def test_a_recorded_replay_plays_every_vehicle_foup_and_tool_as_the_run_had_them(self):
        dataset = smt2020.load_dataset("smat2022")
        config = {"horizon": 730 * DAY, "amhs": {"vehicles": 200}}
        start, end = HOUR // 2, HOUR
        recorded = smt2020.Simulation(dataset, config, {"replay": {"from": start, "until": end}})
        self.assertIsNone(recorded.replay())
        recorded.run(until=end)
        replay = recorded.replay()
        self.assertIsInstance(replay, bytes)
        player = smt2020.ReplayPlayer(dataset, replay)
        self.assertEqual(player.window(), {"from": start, "until": end})
        plain = smt2020.Simulation(dataset, config)
        seen = []
        for at in range(start, end, 61_373):
            plain.run(until=at)
            seen.append((at, plain.amhs()))
        # Played forward, then sought backward.
        for at, amhs in seen + seen[::-1]:
            frame = player.frame(at)
            vehicles, foups = frame["vehicles"], frame["foups"]
            self.assertEqual(len(vehicles["x"]), len(amhs["vehicles"]))
            for id, vehicle in enumerate(amhs["vehicles"]):
                off = math.hypot(vehicle["x"] - vehicles["x"][id], vehicle["y"] - vehicles["y"][id])
                self.assertLess(off, 1.1e-3, f"vehicle {id} at {at}")
                self.assertEqual(vehicles["activity"][id], vehicle["activity"])
            at_ports = {
                lot: index
                for lot, place, index in zip(foups["lot"], foups["place"], foups["index"])
                if place == "port"
            }
            self.assertEqual(at_ports, {foup["lot"]: foup["port"] for foup in amhs["foups"]})
            self.assertEqual(frame["tools"], amhs["tools"])
        with self.assertRaisesRegex(ValueError, "corrupt replay"):
            smt2020.ReplayPlayer(dataset, replay[: len(replay) // 2])
        with self.assertRaisesRegex(ValueError, "no AMHS layout"):
            smt2020.ReplayPlayer(smt2020.load_dataset("ds1"), replay)


if __name__ == "__main__":
    unittest.main()
