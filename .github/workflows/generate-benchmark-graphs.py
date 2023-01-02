import sys
import os
import pandas
import plotly.express as px
#import json
import argparse

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--json-files', required=True) 
    parser.add_argument('--output-html', required=True)
    args = parser.parse_args()

    all_results = []
    for dirpath, dirs, files in os.walk(args.json_files):
        for f in files:
            if f.endswith(".json"):
                f_abs = os.path.join(dirpath, f)
                print(f"Loading {f_abs}...")

                df = pandas.read_json(f_abs)
                print(df)

               # all_results.extend(json.loads(f_abs))
    
   # print(all_results)


if __name__ == "__main__":
    sys.exit(main())