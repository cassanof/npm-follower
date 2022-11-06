#!/bin/bash

BLOB_FILE=./chunk
TO_WRITE=$1 # file to append to the blob
OFFSET_FILE=$2 # file to store the offset,size of the blob

# check if both arguments are provided
if [ $# -ne 2 ]; then
    echo "Usage: $0 <file to append> <offset file>"
    exit 1
fi

# create the blob file if it doesn't exist
if [ ! -f $BLOB_FILE ]; then
    touch $BLOB_FILE
fi

OFFSET=`stat -c %s $BLOB_FILE`



# append to the blob
echo "Appending $TO_WRITE to $BLOB_FILE"
printf "%s" "$(<$TO_WRITE)" >> $BLOB_FILE

# store the offset,size of the blob
SIZE=`stat -c %s $BLOB_FILE`
COUNT=$(($SIZE - $OFFSET))
echo "Offset: $OFFSET"
echo "File size: $SIZE"

echo "$OFFSET,$COUNT" > $OFFSET_FILE
