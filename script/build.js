#!/usr/bin/env node

cp = require('child_process')

let buildDebug = ['-d', '--debug'].includes(process.argv[2])
let releaseFlag = buildDebug ? '' : '--release '
let targetDir = buildDebug ? 'debug' : 'release'

cp.execSync(`cargo rustc ${releaseFlag} -- -Clink-args=\"-undefined dynamic_lookup -export_dynamic\"`, {stdio: 'inherit'})
cp.execSync(`cp target/${targetDir}/{libproton.dylib,proton.node}`, {stdio: 'inherit'})
