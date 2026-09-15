-- Schema captured from baseline 142ff41, before submission payloads existed.
CREATE TABLE tasks(id TEXT PRIMARY KEY,spec TEXT NOT NULL,config_hash TEXT NOT NULL,plan TEXT NOT NULL,status TEXT NOT NULL,total INTEGER NOT NULL,settled INTEGER NOT NULL DEFAULT 0,reserved INTEGER NOT NULL DEFAULT 0,calls INTEGER NOT NULL DEFAULT 0,checkpoint TEXT NOT NULL,result TEXT);
CREATE TABLE attempts(id TEXT PRIMARY KEY,task TEXT NOT NULL REFERENCES tasks(id),amount INTEGER NOT NULL,cost INTEGER,state TEXT NOT NULL,metadata TEXT NOT NULL,outcome TEXT);
CREATE TABLE events(id INTEGER PRIMARY KEY AUTOINCREMENT,task TEXT NOT NULL REFERENCES tasks(id),payload TEXT NOT NULL);
PRAGMA application_id=1296254257;
PRAGMA user_version=1;
