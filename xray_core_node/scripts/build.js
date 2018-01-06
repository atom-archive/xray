#!/usr/bin/env node

const cp = require('child_process')
const path = require('path')

const buildDebug = ['-d', '--debug'].includes(process.argv[2])
const releaseFlag = buildDebug ? '' : '--release '
const targetDir = buildDebug ? 'debug' : 'release'
const moduleName = 'xray_core';

cp.execSync(`cargo rustc ${releaseFlag}--verbose -- -Clink-args=\"-undefined dynamic_lookup -export_dynamic\"`, {stdio: 'inherit'})
cp.execSync(`cp target/${targetDir}/{lib${moduleName}.dylib,${moduleName}.node}`, {stdio: 'inherit'})
