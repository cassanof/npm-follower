#!/bin/bash

BLOB_FILE=$1 # the blob file to write to
TO_WRITE=$2 # file to append to the blob
OFFSET_FILE=$3 # file to store the offset,size of the blob

# check if both arguments are provided
if [ $# -ne 3 ]; then
    echo "Usage: $0 <blob_file> <file_to_append> <offset_file>"
    exit 1
fi

# create the blob file if it doesn't exist
if [ ! -f $BLOB_FILE ]; then
    touch $BLOB_FILE
fi

OFFSET=`stat -c %s $BLOB_FILE`
FILE_SIZE=`stat -c %s $TO_WRITE`
echo "to write: $FILE_SIZE"


# append to the blob
echo "Appending $TO_WRITE to $BLOB_FILE"
dd if=$TO_WRITE of=$BLOB_FILE bs=1 seek=$OFFSET conv=notrunc 2>/dev/null

# store the offset,size of the blob
SIZE=`stat -c %s $BLOB_FILE`
COUNT=$(($SIZE - $OFFSET))
echo "Offset: $OFFSET"

echo "$OFFSET,$COUNT" > $OFFSET_FILE

exit 0
