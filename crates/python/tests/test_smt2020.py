"""The smt2020 module on the bundled DS1: python -m unittest discover -s crates/python/tests"""

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


if __name__ == "__main__":
    unittest.main()
