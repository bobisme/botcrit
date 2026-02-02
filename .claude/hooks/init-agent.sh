#!/bin/bash
AGENT_ID=$(bus whoami --suggest-project-suffix=dev)
echo "Agent ID for use with botbus/crit: $AGENT_ID"
