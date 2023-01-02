import sys
import os
import pandas
import plotly.express as px
import json
import argparse

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--json-files', required=True) 
    parser.add_argument('--output-html', required=True)
    args = parser.parse_args()

    all_results = pandas.DataFrame()
    for dirpath, dirs, files in os.walk(args.json_files):
        for filename in files:
            if filename.endswith(".json"):
                #TODO: parse date/time and commit hash
                filename_abs = os.path.join(dirpath, filename)

                print(f"Loading {filename_abs}...")

                with open(filename_abs, "r") as f:
                    j = json.load(f)

                for target in j:
                    for program in target["results"]:
                        for case in program["results"]:
                            for sample in case["results"]:
                                row = {
                                    "filename": filename, #TODO: put date/time and commit hash here instead
                                    "target-source": target["source"],
                                    "target-dest": target["dest"],
                                    "program": program["program"],
                                    "case": case["case"],
                                    "time": sample["time"],
                                    "peak_memory_local": sample["peak_memory_local"],
                                    "peak_memory_remote": sample["peak_memory_remote"],
                                }
                                all_results = pandas.concat([all_results, pandas.DataFrame.from_records([row], 
                                    index=["filename", "target-source", "target-dest", "program", "case"])])
    
    print(all_results.to_string())


if __name__ == "__main__":
    sys.exit(main())