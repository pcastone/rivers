# Basic Project 

## File System Layout 
```
projectbase/
├── README.md                # Document outline 
├── docs/                    # documents 
│   ├── project_prd.md       # this the project requirement document
│   ├── environment.md       # has the file system layout and archtecture about the project
│   ├── howto.md             # instruction on how to setup and run the project software. 
│   ├── running.md           # instruction on how to quickly run the project. 
│   ├── summary/             # this is where all summary mark down files go. 
│   └── defects/             # where all defect and problem mark down files go. 
├── todo/                    # Public headers
│   ├── tasks.md             # current task to be excuted
│   └── bugfix.md              # this for tracking bugs if the bug has defect (major problem) then 
│                              defects document should be included in the bug list. 
├── logs                     # captures login when code is running from src, code running from
│                              release should use release/logs
├── scripts/                 # location were all script are 
├── src/                     # location of source code and working code this where rust crates go
├── release/                 # final compiled or working code, code should be able to run from code
│                              should use config files in release/ not project config
├── config/                  # location for master config should be copied to release at build time. 
└── scratch/                 # directory for storing temporary files.


