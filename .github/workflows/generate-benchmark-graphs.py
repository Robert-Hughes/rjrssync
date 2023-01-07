#!/usr/bin/env python3

import sys
import os
import pandas
import plotly.express as px
import json
import argparse
import re
from datetime import datetime

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--json-files', required=True) 
    parser.add_argument('--output-html', required=True)
    args = parser.parse_args()

    #e.g. benchmark-results-wsl-2023-01-02-13-26-127ca5f54c8605f9f872548a1157f2b595981863.json
    filename_regex = re.compile("benchmark-results-(.+)-(\d+)-(\d+)-(\d+)-(\d+)-(\d+)-([0-9a-zA-Z]+).json")
    all_results = pandas.DataFrame()
    for dirpath, dirs, files in os.walk(args.json_files):
        for filename in files:
            filename_abs = os.path.join(dirpath, filename)

            match = filename_regex.match(filename)
            if not match:
                print(f"Skipping {filename_abs}")
                continue

            platform = match.group(1)
            timestamp = datetime(int(match.group(2)), int(match.group(3)), int(match.group(4)), int(match.group(5)), int(match.group(6)))
            commit_hash = match.group(7)

            print(f"Loading {filename_abs}...")
            with open(filename_abs, "r") as f:
                j = json.load(f)

            for target in j:
                for program in target["results"]:
                    for case in program["results"]:
                        for sample in case["results"]:
                            for (measurement, value) in sample.items():
                                if value is None:
                                    continue
                                row = {
                                    "platform": platform,
                                    "timestamp": timestamp,
                                    "commit-hash": commit_hash,
                                    "target-source": target["source"],
                                    "target-dest": target["dest"],
                                    # Note that we might have the same source and dest, but running on a different platform
                                    # e.g. 'windows: Remote Linux -> Remote Windows' vs 'wsl: Remote Linux -> Remote Windows'
                                    "target": f"{platform}: {target['source']} -> {target['dest']}",
                                    "program": program["program"],
                                    "case": case["case"],
                                    "measurement": measurement,
                                    "value": value,
                                }
                                all_results = pandas.concat([all_results, pandas.DataFrame.from_records([row])])
    
    print(all_results)

    # fig = px.scatter(all_results, x="timestamp", y="value", facet_row="case", facet_col="measurement",
    #     color="program",
    #     hover_data=["commit-hash"])
    times = all_results.loc[all_results["measurement"] == "time"]
    fig = px.scatter(times, x="timestamp", y="value", facet_row="case", facet_col="target",
        color="program",
        hover_data=["commit-hash"],
        symbol_sequence=["x"])
    fig.update_yaxes(matches=None)
   # fig.layout.yaxis.matches ='y'
  #  fig.layout.yaxis2.matches ='y'
 #   fig.layout.yaxis3.matches ='y'
    fig.for_each_annotation(lambda a: a.update(text=a.text.split("=")[-1]))
    print(f"Writing html to {args.output_html}...")
    fig.write_html(args.output_html)


if __name__ == "__main__":
    sys.exit(main())